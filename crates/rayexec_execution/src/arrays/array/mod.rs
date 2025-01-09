pub mod array_buffer;
pub mod array_data;
pub mod buffer_manager;
pub mod flat;
pub mod physical_type;
pub mod selection;
pub mod string_view;
pub mod validity;

mod raw;
mod shared_or_owned;

use std::fmt::Debug;
use std::sync::Arc;

use array_buffer::{ArrayBuffer, DictionaryBuffer, ListBuffer, SecondaryBuffer};
use array_data::ArrayData;
use buffer_manager::{BufferManager, NopBufferManager};
use flat::FlatArrayView;
use half::f16;
use physical_type::{
    AddressableMut,
    PhysicalAny,
    PhysicalBinary,
    PhysicalBool,
    PhysicalDictionary,
    PhysicalF16,
    PhysicalF32,
    PhysicalF64,
    PhysicalI128,
    PhysicalI16,
    PhysicalI32,
    PhysicalI64,
    PhysicalI8,
    PhysicalInterval,
    PhysicalList,
    PhysicalType,
    PhysicalU128,
    PhysicalU16,
    PhysicalU32,
    PhysicalU64,
    PhysicalU8,
    PhysicalUntypedNull,
    PhysicalUtf8,
};
use rayexec_error::{not_implemented, RayexecError, Result, ResultExt};
use shared_or_owned::SharedOrOwned;
use stdutil::iter::TryFromExactSizeIterator;
use string_view::StringViewHeap;
use validity::Validity;

use crate::arrays::bitmap::Bitmap;
use crate::arrays::datatype::DataType;
use crate::arrays::executor::scalar::UnaryExecutor;
use crate::arrays::scalar::decimal::{Decimal128Scalar, Decimal64Scalar};
use crate::arrays::scalar::interval::Interval;
use crate::arrays::scalar::timestamp::TimestampScalar;
use crate::arrays::scalar::ScalarValue;
use crate::arrays::selection::SelectionVector;
use crate::arrays::storage::{
    AddressableStorage,
    BooleanStorage,
    ContiguousVarlenStorage,
    GermanVarlenStorage,
    ListStorage,
    PrimitiveStorage,
    UntypedNullStorage,
};

/// Validity mask for physical storage.
pub type PhysicalValidity = SharedOrOwned<Bitmap>;

/// Logical row selection.
pub type LogicalSelection = SharedOrOwned<SelectionVector>;

#[derive(Debug)]
pub(crate) struct ArrayNextInner<B: BufferManager> {
    pub(crate) validity: Validity,
    pub(crate) data: ArrayData<B>,
}

// TODO: Remove Clone, PartialEq
#[derive(Debug)]
pub struct Array<B: BufferManager = NopBufferManager> {
    /// Data type of the array.
    pub(crate) datatype: DataType,
    /// Selection of rows for the array.
    ///
    /// If set, this provides logical row mapping on top of the underlying data.
    /// If not set, then there's a one-to-one mapping between the logical row
    /// and and row in the underlying data.
    // TODO: Remove
    pub(crate) selection2: Option<LogicalSelection>,
    /// Option validity mask.
    ///
    /// This indicates the validity of the underlying data. This does not take
    /// into account the selection vector, and always maps directly to the data.
    // TODO: Remove
    pub(crate) validity2: Option<PhysicalValidity>,
    /// The physical data.
    // TODO: Remove
    pub(crate) data2: ArrayData2,

    /// Contents of the refactored array internals.
    ///
    /// Will be flattened and `selection2`, `validity2`, `data2` will be removed
    /// once everything's switched over.
    pub(crate) next: Option<ArrayNextInner<B>>,
}

// TODO: Remove
impl Clone for Array {
    fn clone(&self) -> Self {
        Array {
            datatype: self.datatype.clone(),
            selection2: self.selection2.clone(),
            validity2: self.validity2.clone(),
            data2: self.data2.clone(),
            next: None,
        }
    }
}

// TODO: Remove
impl PartialEq for Array {
    fn eq(&self, other: &Self) -> bool {
        self.datatype == other.datatype
            && self.selection2 == other.selection2
            && self.validity2 == other.validity2
            && self.data2 == other.data2
    }
}

impl<B> Array<B>
where
    B: BufferManager,
{
    /// Create a new array with the given capacity.
    ///
    /// This will take care of initalizing the primary and secondary data
    /// buffers depending on the type.
    pub fn try_new(manager: &Arc<B>, datatype: DataType, capacity: usize) -> Result<Self> {
        let buffer = array_buffer_for_datatype(manager, &datatype, capacity)?;
        let validity = Validity::new_all_valid(capacity);

        Ok(Array {
            datatype,
            selection2: None,
            validity2: None,
            data2: ArrayData2::UntypedNull(UntypedNullStorage(capacity)),
            next: Some(ArrayNextInner {
                validity,
                data: ArrayData::owned(buffer),
            }),
        })
    }

    // TODO: Remove
    #[allow(dead_code)]
    pub(crate) fn next(&self) -> &ArrayNextInner<B> {
        self.next.as_ref().expect("next to be set")
    }

    // TODO: Remove
    #[allow(dead_code)]
    pub(crate) fn next_mut(&mut self) -> &mut ArrayNextInner<B> {
        self.next.as_mut().expect("next to be set")
    }

    pub fn capacity(&self) -> usize {
        if let Some(next) = &self.next {
            return next.data.primary_capacity();
        }

        // TODO: Remove, just using to not break things completely yet.
        match self.selection2.as_ref().map(|v| v.as_ref()) {
            Some(v) => v.num_rows(),
            None => self.data2.len(),
        }
    }

    pub fn datatype(&self) -> &DataType {
        &self.datatype
    }

    pub fn put_validity(&mut self, validity: Validity) -> Result<()> {
        let next = self.next_mut();

        if validity.len() != next.data.primary_capacity() {
            return Err(RayexecError::new("Invalid validity length")
                .with_field("got", validity.len())
                .with_field("want", next.data.primary_capacity()));
        }
        next.validity = validity;

        Ok(())
    }

    pub fn is_dictionary(&self) -> bool {
        self.next.as_ref().unwrap().data.physical_type() == PhysicalType::Dictionary
    }

    pub fn flat_view(&self) -> Result<FlatArrayView<B>> {
        FlatArrayView::from_array(self)
    }

    /// Selects indice from the array.
    ///
    /// This will convert the underlying array buffer into a dictionary buffer.
    pub fn select(
        &mut self,
        manager: &Arc<B>,
        selection: impl stdutil::iter::IntoExactSizeIterator<Item = usize>,
    ) -> Result<()> {
        let is_dictionary = self.is_dictionary();
        let next = self.next_mut();

        if is_dictionary {
            // Already dictionary, select the selection.
            let sel = selection.into_iter();
            let mut new_buf =
                ArrayBuffer::with_primary_capacity::<PhysicalDictionary>(manager, sel.len())?;

            let old_sel = next.data.try_as_slice::<PhysicalDictionary>()?;
            let new_sel = new_buf.try_as_slice_mut::<PhysicalDictionary>()?;

            for (sel_idx, sel_buf) in sel.zip(new_sel) {
                let idx = old_sel[sel_idx];
                *sel_buf = idx;
            }

            // Now swap the secondary buffers, the dictionary buffer will now be
            // on `new_buf`.
            std::mem::swap(
                next.data.try_as_mut()?.get_secondary_mut(), // TODO: Should just clone the pointer if managed.
                new_buf.get_secondary_mut(),
            );

            // And set the new buf, old buf gets dropped.
            next.data = ArrayData::owned(new_buf);

            debug_assert!(matches!(
                next.data.get_secondary(),
                SecondaryBuffer::Dictionary(_)
            ));

            return Ok(());
        }

        let sel = selection.into_iter();
        let mut new_buf =
            ArrayBuffer::with_primary_capacity::<PhysicalDictionary>(manager, sel.len())?;

        let new_buf_slice = new_buf.try_as_slice_mut::<PhysicalDictionary>()?;

        // Set all selection indices in the new array buffer.
        for (sel_idx, sel_buf) in sel.zip(new_buf_slice) {
            *sel_buf = sel_idx
        }

        // TODO: Probably verify selection all in bounds.

        // Now replace the original buffer, and put the original buffer in the
        // secondary buffer.
        let orig_validity = std::mem::replace(
            &mut next.validity,
            Validity::new_all_valid(new_buf.primary_capacity()),
        );
        let orig_buffer = std::mem::replace(&mut next.data, ArrayData::owned(new_buf));
        // TODO: Should just clone the pointer if managed.
        next.data
            .try_as_mut()?
            .put_secondary_buffer(SecondaryBuffer::Dictionary(DictionaryBuffer {
                validity: orig_validity,
                buffer: orig_buffer,
            }));

        debug_assert!(matches!(
            next.data.get_secondary(),
            SecondaryBuffer::Dictionary(_)
        ));

        Ok(())
    }
}

impl Array {
    pub fn new_untyped_null_array(len: usize) -> Self {
        // Note that we're adding a bitmap here even though the data already
        // returns NULL. This allows the executors (especially for aggregates)
        // to solely look at the bitmap to determine if a row should executed
        // on.
        let validity = Bitmap::new_with_all_false(1);
        let selection = SelectionVector::repeated(len, 0);
        let data = UntypedNullStorage(1);

        Array {
            datatype: DataType::Null,
            selection2: Some(selection.into()),
            validity2: Some(validity.into()),
            data2: data.into(),
            next: None,
        }
    }

    /// Creates a new typed array with all values being set to null.
    pub fn new_typed_null_array(datatype: DataType, len: usize) -> Result<Self> {
        // Create physical array data of length 1, and use a selection vector to
        // extend it out to the desired size.
        let data = datatype.physical_type()?.zeroed_array_data(1);
        let validity = Bitmap::new_with_all_false(1);
        let selection = SelectionVector::repeated(len, 0);

        Ok(Array {
            datatype,
            selection2: Some(selection.into()),
            validity2: Some(validity.into()),
            data2: data,
            next: None,
        })
    }

    pub fn new_with_array_data(datatype: DataType, data: impl Into<ArrayData2>) -> Self {
        Array {
            datatype,
            selection2: None,
            validity2: None,
            data2: data.into(),
            next: None,
        }
    }

    pub fn new_with_validity_and_array_data(
        datatype: DataType,
        validity: impl Into<PhysicalValidity>,
        data: impl Into<ArrayData2>,
    ) -> Self {
        Array {
            datatype,
            selection2: None,
            validity2: Some(validity.into()),
            data2: data.into(),
            next: None,
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selection2.is_some()
    }

    pub fn selection_vector(&self) -> Option<&SelectionVector> {
        self.selection2.as_ref().map(|v| v.as_ref())
    }

    /// Sets the validity for a value at a given physical index.
    pub fn set_physical_validity(&mut self, idx: usize, valid: bool) {
        match &mut self.validity2 {
            Some(validity) => {
                let validity = validity.get_mut();
                validity.set_unchecked(idx, valid);
            }
            None => {
                // Initialize validity.
                let len = self.data2.len();
                let mut validity = Bitmap::new_with_all_true(len);
                validity.set_unchecked(idx, valid);

                self.validity2 = Some(validity.into())
            }
        }
    }

    // TODO: Validating variant too.
    pub fn put_selection(&mut self, selection: impl Into<LogicalSelection>) {
        self.selection2 = Some(selection.into())
    }

    pub fn make_shared(&mut self) {
        if let Some(validity) = &mut self.validity2 {
            validity.make_shared();
        }
        if let Some(selection) = &mut self.selection2 {
            selection.make_shared()
        }
    }

    /// Updates this array's selection vector.
    ///
    /// Takes into account any existing selection. This allows for repeated
    /// selection (filtering) against the same array.
    // TODO: Add test for selecting on logically empty array.
    pub fn select_mut2(&mut self, selection: impl Into<LogicalSelection>) {
        let selection = selection.into();
        match self.selection_vector() {
            Some(existing) => {
                let selection = existing.select(selection.as_ref());
                self.selection2 = Some(selection.into())
            }
            None => {
                // No existing selection, we can just use the provided vector
                // directly.
                self.selection2 = Some(selection)
            }
        }
    }

    pub fn logical_len(&self) -> usize {
        match self.selection_vector() {
            Some(v) => v.num_rows(),
            None => self.data2.len(),
        }
    }

    pub fn validity(&self) -> Option<&Bitmap> {
        self.validity2.as_ref().map(|v| v.as_ref())
    }

    pub fn is_valid(&self, idx: usize) -> Option<bool> {
        if idx >= self.logical_len() {
            return None;
        }

        let idx = match self.selection_vector() {
            Some(v) => v.get_opt(idx)?,
            None => idx,
        };

        if let Some(validity) = &self.validity2 {
            return Some(validity.as_ref().value(idx));
        }

        Some(true)
    }

    /// Returns the array data.
    ///
    /// ArrayData can be cheaply cloned.
    pub fn array_data(&self) -> &ArrayData2 {
        &self.data2
    }

    pub fn into_array_data(self) -> ArrayData2 {
        self.data2
    }

    /// Gets the physical type of the array.
    pub fn physical_type(&self) -> PhysicalType {
        match self.data2.physical_type() {
            PhysicalType::Binary => match self.datatype {
                DataType::Utf8 => PhysicalType::Utf8,
                _ => PhysicalType::Binary,
            },
            other => other,
        }
    }

    /// Get the value at a logical index.
    ///
    /// Takes into account the validity and selection vector.
    pub fn logical_value(&self, idx: usize) -> Result<ScalarValue> {
        let idx = match self.selection_vector() {
            Some(v) => v
                .get_opt(idx)
                .ok_or_else(|| RayexecError::new(format!("Logical index {idx} out of bounds")))?,
            None => idx,
        };

        if let Some(validity) = &self.validity2 {
            if !validity.as_ref().value(idx) {
                return Ok(ScalarValue::Null);
            }
        }

        self.physical_scalar(idx)
    }

    /// Gets the scalar value at the physical index.
    ///
    /// Ignores validity and selectivitity.
    pub fn physical_scalar(&self, idx: usize) -> Result<ScalarValue> {
        Ok(match &self.datatype {
            DataType::Null => match &self.data2 {
                ArrayData2::UntypedNull(_) => ScalarValue::Null,
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Boolean => match &self.data2 {
                ArrayData2::Boolean(arr) => arr.as_ref().as_ref().value(idx).into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Float16 => match &self.data2 {
                ArrayData2::Float16(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Float32 => match &self.data2 {
                ArrayData2::Float32(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Float64 => match &self.data2 {
                ArrayData2::Float64(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Int8 => match &self.data2 {
                ArrayData2::Int8(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Int16 => match &self.data2 {
                ArrayData2::Int16(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Int32 => match &self.data2 {
                ArrayData2::Int32(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Int64 => match &self.data2 {
                ArrayData2::Int64(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Int128 => match &self.data2 {
                ArrayData2::Int64(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::UInt8 => match &self.data2 {
                ArrayData2::UInt8(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::UInt16 => match &self.data2 {
                ArrayData2::UInt16(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::UInt32 => match &self.data2 {
                ArrayData2::UInt32(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::UInt64 => match &self.data2 {
                ArrayData2::UInt64(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::UInt128 => match &self.data2 {
                ArrayData2::UInt64(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Decimal64(m) => match &self.data2 {
                ArrayData2::Int64(arr) => ScalarValue::Decimal64(Decimal64Scalar {
                    precision: m.precision,
                    scale: m.scale,
                    value: arr.as_ref().as_ref()[idx],
                }),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Decimal128(m) => match &self.data2 {
                ArrayData2::Int128(arr) => ScalarValue::Decimal128(Decimal128Scalar {
                    precision: m.precision,
                    scale: m.scale,
                    value: arr.as_ref().as_ref()[idx],
                }),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Date32 => match &self.data2 {
                ArrayData2::Int32(arr) => ScalarValue::Date32(arr.as_ref().as_ref()[idx]),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Date64 => match &self.data2 {
                ArrayData2::Int64(arr) => ScalarValue::Date64(arr.as_ref().as_ref()[idx]),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Timestamp(m) => match &self.data2 {
                ArrayData2::Int64(arr) => ScalarValue::Timestamp(TimestampScalar {
                    unit: m.unit,
                    value: arr.as_ref().as_ref()[idx],
                }),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Interval => match &self.data2 {
                ArrayData2::Interval(arr) => arr.as_ref().as_ref()[idx].into(),
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
            DataType::Utf8 => {
                let v = match &self.data2 {
                    ArrayData2::Binary(BinaryData::Binary(arr)) => arr
                        .get(idx)
                        .ok_or_else(|| RayexecError::new("missing data"))?,
                    ArrayData2::Binary(BinaryData::LargeBinary(arr)) => arr
                        .get(idx)
                        .ok_or_else(|| RayexecError::new("missing data"))?,
                    ArrayData2::Binary(BinaryData::German(arr)) => arr
                        .get(idx)
                        .ok_or_else(|| RayexecError::new("missing data"))?,
                    _other => return Err(array_not_valid_for_type_err(&self.datatype)),
                };
                let s = std::str::from_utf8(v).context("binary data not valid utf8")?;
                s.into()
            }
            DataType::Binary => {
                let v = match &self.data2 {
                    ArrayData2::Binary(BinaryData::Binary(arr)) => arr
                        .get(idx)
                        .ok_or_else(|| RayexecError::new("missing data"))?,
                    ArrayData2::Binary(BinaryData::LargeBinary(arr)) => arr
                        .get(idx)
                        .ok_or_else(|| RayexecError::new("missing data"))?,
                    ArrayData2::Binary(BinaryData::German(arr)) => arr
                        .get(idx)
                        .ok_or_else(|| RayexecError::new("missing data"))?,
                    _other => return Err(array_not_valid_for_type_err(&self.datatype)),
                };
                v.into()
            }
            DataType::Struct(_) => not_implemented!("get value: struct"),
            DataType::List(_) => match &self.data2 {
                ArrayData2::List(list) => {
                    let meta = list
                        .metadata
                        .as_slice()
                        .get(idx)
                        .ok_or_else(|| RayexecError::new("Out of bounds"))?;

                    let vals = (meta.offset..meta.offset + meta.len)
                        .map(|idx| list.array.physical_scalar(idx as usize))
                        .collect::<Result<Vec<_>>>()?;

                    ScalarValue::List(vals)
                }
                _other => return Err(array_not_valid_for_type_err(&self.datatype)),
            },
        })
    }

    /// Checks if a scalar value is logically equal to a value in the array.
    pub fn scalar_value_logically_eq(&self, scalar: &ScalarValue, row: usize) -> Result<bool> {
        if row >= self.logical_len() {
            return Err(RayexecError::new("Row out of bounds"));
        }

        match scalar {
            ScalarValue::Null => {
                UnaryExecutor::value_at2::<PhysicalAny>(self, row).map(|arr_val| arr_val.is_none())
            } // None == NULL
            ScalarValue::Boolean(v) => {
                UnaryExecutor::value_at2::<PhysicalBool>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Int8(v) => {
                UnaryExecutor::value_at2::<PhysicalI8>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Int16(v) => {
                UnaryExecutor::value_at2::<PhysicalI16>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Int32(v) => {
                UnaryExecutor::value_at2::<PhysicalI32>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Int64(v) => {
                UnaryExecutor::value_at2::<PhysicalI64>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Int128(v) => {
                UnaryExecutor::value_at2::<PhysicalI128>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::UInt8(v) => {
                UnaryExecutor::value_at2::<PhysicalU8>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::UInt16(v) => {
                UnaryExecutor::value_at2::<PhysicalU16>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::UInt32(v) => {
                UnaryExecutor::value_at2::<PhysicalU32>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::UInt64(v) => {
                UnaryExecutor::value_at2::<PhysicalU64>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::UInt128(v) => {
                UnaryExecutor::value_at2::<PhysicalU128>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Float32(v) => {
                UnaryExecutor::value_at2::<PhysicalF32>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Float64(v) => {
                UnaryExecutor::value_at2::<PhysicalF64>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Date32(v) => {
                UnaryExecutor::value_at2::<PhysicalI32>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Date64(v) => {
                UnaryExecutor::value_at2::<PhysicalI64>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                })
            }
            ScalarValue::Interval(v) => UnaryExecutor::value_at2::<PhysicalInterval>(self, row)
                .map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == *v,
                    None => false,
                }),
            ScalarValue::Utf8(v) => {
                UnaryExecutor::value_at2::<PhysicalUtf8>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == v.as_ref(),
                    None => false,
                })
            }
            ScalarValue::Binary(v) => {
                UnaryExecutor::value_at2::<PhysicalBinary>(self, row).map(|arr_val| match arr_val {
                    Some(arr_val) => arr_val == v.as_ref(),
                    None => false,
                })
            }
            ScalarValue::Timestamp(v) => {
                UnaryExecutor::value_at2::<PhysicalI64>(self, row).map(|arr_val| {
                    // Assumes time unit is the same
                    match arr_val {
                        Some(arr_val) => arr_val == v.value,
                        None => false,
                    }
                })
            }
            ScalarValue::Decimal64(v) => {
                UnaryExecutor::value_at2::<PhysicalI64>(self, row).map(|arr_val| {
                    // Assumes precision/scale are the same.
                    match arr_val {
                        Some(arr_val) => arr_val == v.value,
                        None => false,
                    }
                })
            }
            ScalarValue::Decimal128(v) => {
                UnaryExecutor::value_at2::<PhysicalI128>(self, row).map(|arr_val| {
                    // Assumes precision/scale are the same.
                    match arr_val {
                        Some(arr_val) => arr_val == v.value,
                        None => false,
                    }
                })
            }

            other => not_implemented!("scalar value eq: {other}"),
        }
    }

    pub fn try_slice(&self, offset: usize, count: usize) -> Result<Self> {
        if offset + count > self.logical_len() {
            return Err(RayexecError::new("Slice out of bounds"));
        }
        Ok(self.slice(offset, count))
    }

    pub fn slice(&self, offset: usize, count: usize) -> Self {
        let selection = match self.selection_vector() {
            Some(sel) => sel.slice_unchecked(offset, count),
            None => SelectionVector::with_range(offset..(offset + count)),
        };

        Array {
            datatype: self.datatype.clone(),
            selection2: Some(selection.into()),
            validity2: self.validity2.clone(),
            data2: self.data2.clone(),
            next: None,
        }
    }
}

fn array_not_valid_for_type_err(datatype: &DataType) -> RayexecError {
    RayexecError::new(format!("Array data not valid for data type: {datatype}"))
}

impl<F> FromIterator<Option<F>> for Array
where
    F: Default,
    Array: FromIterator<F>,
{
    fn from_iter<T: IntoIterator<Item = Option<F>>>(iter: T) -> Self {
        // TODO: Make a bit more performant, this is used for more than just
        // tests now.
        let vals: Vec<_> = iter.into_iter().collect();
        let mut validity = Bitmap::new_with_all_true(vals.len());

        let mut new_vals = Vec::with_capacity(vals.len());
        for (idx, val) in vals.into_iter().enumerate() {
            match val {
                Some(val) => new_vals.push(val),
                None => {
                    new_vals.push(F::default());
                    validity.set_unchecked(idx, false);
                }
            }
        }

        let mut array = Array::from_iter(new_vals);
        array.validity2 = Some(validity.into());

        array
    }
}

impl FromIterator<String> for Array {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut german = GermanVarlenStorage::with_metadata_capacity(lower);

        for s in iter {
            german.try_push(s.as_bytes()).unwrap();
        }

        Array {
            datatype: DataType::Utf8,
            selection2: None,
            validity2: None,
            data2: ArrayData2::Binary(BinaryData::German(Arc::new(german))),
            next: None,
        }
    }
}

impl<'a> FromIterator<&'a str> for Array {
    fn from_iter<T: IntoIterator<Item = &'a str>>(iter: T) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut german = GermanVarlenStorage::with_metadata_capacity(lower);

        for s in iter {
            german.try_push(s.as_bytes()).unwrap();
        }

        Array {
            datatype: DataType::Utf8,
            selection2: None,
            validity2: None,
            data2: ArrayData2::Binary(BinaryData::German(Arc::new(german))),
            next: None,
        }
    }
}

macro_rules! impl_primitive_from_iter {
    ($prim:ty, $variant:ident) => {
        impl FromIterator<$prim> for Array {
            fn from_iter<T: IntoIterator<Item = $prim>>(iter: T) -> Self {
                let vals: Vec<_> = iter.into_iter().collect();
                Array {
                    datatype: DataType::$variant,
                    selection2: None,
                    validity2: None,
                    data2: ArrayData2::$variant(Arc::new(vals.into())),
                    next: None,
                }
            }
        }
    };
}

impl_primitive_from_iter!(i8, Int8);
impl_primitive_from_iter!(i16, Int16);
impl_primitive_from_iter!(i32, Int32);
impl_primitive_from_iter!(i64, Int64);
impl_primitive_from_iter!(i128, Int128);
impl_primitive_from_iter!(u8, UInt8);
impl_primitive_from_iter!(u16, UInt16);
impl_primitive_from_iter!(u32, UInt32);
impl_primitive_from_iter!(u64, UInt64);
impl_primitive_from_iter!(u128, UInt128);
impl_primitive_from_iter!(f16, Float16);
impl_primitive_from_iter!(f32, Float32);
impl_primitive_from_iter!(f64, Float64);

impl FromIterator<bool> for Array {
    fn from_iter<T: IntoIterator<Item = bool>>(iter: T) -> Self {
        let vals: Bitmap = iter.into_iter().collect();
        Array {
            datatype: DataType::Boolean,
            selection2: None,
            validity2: None,
            data2: ArrayData2::Boolean(Arc::new(vals.into())),
            next: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrayData2 {
    UntypedNull(UntypedNullStorage),
    Boolean(Arc<BooleanStorage>),
    Float16(Arc<PrimitiveStorage<f16>>),
    Float32(Arc<PrimitiveStorage<f32>>),
    Float64(Arc<PrimitiveStorage<f64>>),
    Int8(Arc<PrimitiveStorage<i8>>),
    Int16(Arc<PrimitiveStorage<i16>>),
    Int32(Arc<PrimitiveStorage<i32>>),
    Int64(Arc<PrimitiveStorage<i64>>),
    Int128(Arc<PrimitiveStorage<i128>>),
    UInt8(Arc<PrimitiveStorage<u8>>),
    UInt16(Arc<PrimitiveStorage<u16>>),
    UInt32(Arc<PrimitiveStorage<u32>>),
    UInt64(Arc<PrimitiveStorage<u64>>),
    UInt128(Arc<PrimitiveStorage<u128>>),
    Interval(Arc<PrimitiveStorage<Interval>>),
    Binary(BinaryData),
    List(Arc<ListStorage>),
}

impl ArrayData2 {
    pub fn physical_type(&self) -> PhysicalType {
        match self {
            Self::UntypedNull(_) => PhysicalType::UntypedNull,
            Self::Boolean(_) => PhysicalType::Boolean,
            Self::Float16(_) => PhysicalType::Float16,
            Self::Float32(_) => PhysicalType::Float32,
            Self::Float64(_) => PhysicalType::Float64,
            Self::Int8(_) => PhysicalType::Int8,
            Self::Int16(_) => PhysicalType::Int16,
            Self::Int32(_) => PhysicalType::Int32,
            Self::Int64(_) => PhysicalType::Int64,
            Self::Int128(_) => PhysicalType::Int128,
            Self::UInt8(_) => PhysicalType::UInt8,
            Self::UInt16(_) => PhysicalType::UInt16,
            Self::UInt32(_) => PhysicalType::UInt32,
            Self::UInt64(_) => PhysicalType::UInt64,
            Self::UInt128(_) => PhysicalType::UInt128,
            Self::Interval(_) => PhysicalType::Interval,
            Self::Binary(_) => PhysicalType::Binary,
            Self::List(_) => PhysicalType::List,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::UntypedNull(s) => s.len(),
            Self::Boolean(s) => s.len(),
            Self::Float16(s) => s.len(),
            Self::Float32(s) => s.len(),
            Self::Float64(s) => s.len(),
            Self::Int8(s) => s.len(),
            Self::Int16(s) => s.len(),
            Self::Int32(s) => s.len(),
            Self::Int64(s) => s.len(),
            Self::Int128(s) => s.len(),
            Self::UInt8(s) => s.len(),
            Self::UInt16(s) => s.len(),
            Self::UInt32(s) => s.len(),
            Self::UInt64(s) => s.len(),
            Self::UInt128(s) => s.len(),
            Self::Interval(s) => s.len(),
            Self::Binary(bin) => match bin {
                BinaryData::Binary(s) => s.len(),
                BinaryData::LargeBinary(s) => s.len(),
                BinaryData::German(s) => s.len(),
            },
            ArrayData2::List(s) => s.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinaryData {
    Binary(Arc<ContiguousVarlenStorage<i32>>),
    LargeBinary(Arc<ContiguousVarlenStorage<i64>>),
    German(Arc<GermanVarlenStorage>),
}

impl BinaryData {
    /// Get the binary data size for the array.
    ///
    /// This will not include metadata size in the calculation.
    pub fn binary_data_size_bytes(&self) -> usize {
        match self {
            Self::Binary(s) => s.data_size_bytes(),
            Self::LargeBinary(s) => s.data_size_bytes(),
            Self::German(s) => s.data_size_bytes(),
        }
    }
}

impl From<UntypedNullStorage> for ArrayData2 {
    fn from(value: UntypedNullStorage) -> Self {
        ArrayData2::UntypedNull(value)
    }
}

impl From<BooleanStorage> for ArrayData2 {
    fn from(value: BooleanStorage) -> Self {
        ArrayData2::Boolean(value.into())
    }
}

impl From<PrimitiveStorage<f16>> for ArrayData2 {
    fn from(value: PrimitiveStorage<f16>) -> Self {
        ArrayData2::Float16(value.into())
    }
}

impl From<PrimitiveStorage<f32>> for ArrayData2 {
    fn from(value: PrimitiveStorage<f32>) -> Self {
        ArrayData2::Float32(value.into())
    }
}

impl From<PrimitiveStorage<f64>> for ArrayData2 {
    fn from(value: PrimitiveStorage<f64>) -> Self {
        ArrayData2::Float64(value.into())
    }
}

impl From<PrimitiveStorage<i8>> for ArrayData2 {
    fn from(value: PrimitiveStorage<i8>) -> Self {
        ArrayData2::Int8(value.into())
    }
}

impl From<PrimitiveStorage<i16>> for ArrayData2 {
    fn from(value: PrimitiveStorage<i16>) -> Self {
        ArrayData2::Int16(value.into())
    }
}

impl From<PrimitiveStorage<i32>> for ArrayData2 {
    fn from(value: PrimitiveStorage<i32>) -> Self {
        ArrayData2::Int32(value.into())
    }
}

impl From<PrimitiveStorage<i64>> for ArrayData2 {
    fn from(value: PrimitiveStorage<i64>) -> Self {
        ArrayData2::Int64(value.into())
    }
}

impl From<PrimitiveStorage<i128>> for ArrayData2 {
    fn from(value: PrimitiveStorage<i128>) -> Self {
        ArrayData2::Int128(value.into())
    }
}

impl From<PrimitiveStorage<u8>> for ArrayData2 {
    fn from(value: PrimitiveStorage<u8>) -> Self {
        ArrayData2::UInt8(value.into())
    }
}

impl From<PrimitiveStorage<u16>> for ArrayData2 {
    fn from(value: PrimitiveStorage<u16>) -> Self {
        ArrayData2::UInt16(value.into())
    }
}

impl From<PrimitiveStorage<u32>> for ArrayData2 {
    fn from(value: PrimitiveStorage<u32>) -> Self {
        ArrayData2::UInt32(value.into())
    }
}

impl From<PrimitiveStorage<u64>> for ArrayData2 {
    fn from(value: PrimitiveStorage<u64>) -> Self {
        ArrayData2::UInt64(value.into())
    }
}

impl From<PrimitiveStorage<u128>> for ArrayData2 {
    fn from(value: PrimitiveStorage<u128>) -> Self {
        ArrayData2::UInt128(value.into())
    }
}

impl From<PrimitiveStorage<Interval>> for ArrayData2 {
    fn from(value: PrimitiveStorage<Interval>) -> Self {
        ArrayData2::Interval(value.into())
    }
}

impl From<GermanVarlenStorage> for ArrayData2 {
    fn from(value: GermanVarlenStorage) -> Self {
        ArrayData2::Binary(BinaryData::German(Arc::new(value)))
    }
}

impl From<ListStorage> for ArrayData2 {
    fn from(value: ListStorage) -> Self {
        ArrayData2::List(Arc::new(value))
    }
}

/// Create a new array buffer for a datatype.
fn array_buffer_for_datatype<B>(
    manager: &Arc<B>,
    datatype: &DataType,
    capacity: usize,
) -> Result<ArrayBuffer<B>>
where
    B: BufferManager,
{
    let buffer = match datatype.physical_type()? {
        PhysicalType::UntypedNull => {
            ArrayBuffer::with_primary_capacity::<PhysicalUntypedNull>(manager, capacity)?
        }
        PhysicalType::Boolean => {
            ArrayBuffer::with_primary_capacity::<PhysicalBool>(manager, capacity)?
        }
        PhysicalType::Int8 => ArrayBuffer::with_primary_capacity::<PhysicalI8>(manager, capacity)?,
        PhysicalType::Int16 => {
            ArrayBuffer::with_primary_capacity::<PhysicalI16>(manager, capacity)?
        }
        PhysicalType::Int32 => {
            ArrayBuffer::with_primary_capacity::<PhysicalI32>(manager, capacity)?
        }
        PhysicalType::Int64 => {
            ArrayBuffer::with_primary_capacity::<PhysicalI64>(manager, capacity)?
        }
        PhysicalType::Int128 => {
            ArrayBuffer::with_primary_capacity::<PhysicalI128>(manager, capacity)?
        }
        PhysicalType::UInt8 => ArrayBuffer::with_primary_capacity::<PhysicalU8>(manager, capacity)?,
        PhysicalType::UInt16 => {
            ArrayBuffer::with_primary_capacity::<PhysicalU16>(manager, capacity)?
        }
        PhysicalType::UInt32 => {
            ArrayBuffer::with_primary_capacity::<PhysicalU32>(manager, capacity)?
        }
        PhysicalType::UInt64 => {
            ArrayBuffer::with_primary_capacity::<PhysicalU64>(manager, capacity)?
        }
        PhysicalType::UInt128 => {
            ArrayBuffer::with_primary_capacity::<PhysicalU128>(manager, capacity)?
        }
        PhysicalType::Float16 => {
            ArrayBuffer::with_primary_capacity::<PhysicalF16>(manager, capacity)?
        }
        PhysicalType::Float32 => {
            ArrayBuffer::with_primary_capacity::<PhysicalF32>(manager, capacity)?
        }
        PhysicalType::Float64 => {
            ArrayBuffer::with_primary_capacity::<PhysicalF64>(manager, capacity)?
        }
        PhysicalType::Interval => {
            ArrayBuffer::with_primary_capacity::<PhysicalInterval>(manager, capacity)?
        }
        PhysicalType::Utf8 => {
            let mut buffer = ArrayBuffer::with_primary_capacity::<PhysicalUtf8>(manager, capacity)?;
            buffer.put_secondary_buffer(SecondaryBuffer::StringViewHeap(StringViewHeap::new()));
            buffer
        }
        PhysicalType::List => {
            let inner_type = match &datatype {
                DataType::List(m) => m.datatype.as_ref().clone(),
                other => {
                    return Err(RayexecError::new(format!(
                        "Expected list datatype, got {other}"
                    )))
                }
            };

            let child = Array::try_new(manager, inner_type, capacity)?;

            let mut buffer = ArrayBuffer::with_primary_capacity::<PhysicalList>(manager, capacity)?;
            buffer.put_secondary_buffer(SecondaryBuffer::List(ListBuffer::new(child)));

            buffer
        }
        other => not_implemented!("create array buffer for physical type {other}"),
    };

    Ok(buffer)
}

/// Implements `try_from_iter` for primitive types.
///
/// Note these create arrays using Nop buffer manager and so really only
/// suitable for tests right now.
macro_rules! impl_primitive_from_iter {
    ($prim:ty, $phys:ty, $typ_variant:ident) => {
        impl TryFromExactSizeIterator<$prim> for Array {
            type Error = RayexecError;

            fn try_from_iter<T: stdutil::iter::IntoExactSizeIterator<Item = $prim>>(
                iter: T,
            ) -> Result<Self, Self::Error> {
                let iter = iter.into_iter();

                let manager = Arc::new(NopBufferManager);

                let mut array = Array::try_new(&manager, DataType::$typ_variant, iter.len())?;
                let slice = array
                    .next
                    .as_mut()
                    .unwrap()
                    .data
                    .try_as_mut()?
                    .try_as_slice_mut::<$phys>()?;

                for (dest, v) in slice.iter_mut().zip(iter) {
                    *dest = v;
                }

                Ok(array)
            }
        }
    };
}

// TODO: Bool

impl_primitive_from_iter!(i8, PhysicalI8, Int8);
impl_primitive_from_iter!(i16, PhysicalI16, Int16);
impl_primitive_from_iter!(i32, PhysicalI32, Int32);
impl_primitive_from_iter!(i64, PhysicalI64, Int64);
impl_primitive_from_iter!(i128, PhysicalI128, Int128);

impl_primitive_from_iter!(u8, PhysicalU8, UInt8);
impl_primitive_from_iter!(u16, PhysicalU16, UInt16);
impl_primitive_from_iter!(u32, PhysicalU32, UInt32);
impl_primitive_from_iter!(u64, PhysicalU64, UInt64);
impl_primitive_from_iter!(u128, PhysicalU128, UInt128);

impl_primitive_from_iter!(f16, PhysicalF16, Float16);
impl_primitive_from_iter!(f32, PhysicalF32, Float32);
impl_primitive_from_iter!(f64, PhysicalF64, Float64);

impl_primitive_from_iter!(Interval, PhysicalInterval, Interval);

impl<'a> TryFromExactSizeIterator<&'a str> for Array<NopBufferManager> {
    type Error = RayexecError;

    fn try_from_iter<T: stdutil::iter::IntoExactSizeIterator<Item = &'a str>>(
        iter: T,
    ) -> Result<Self, Self::Error> {
        let iter = iter.into_iter();
        let len = iter.len();

        let mut buffer =
            ArrayBuffer::with_primary_capacity::<PhysicalUtf8>(&Arc::new(NopBufferManager), len)?;
        buffer.put_secondary_buffer(SecondaryBuffer::StringViewHeap(StringViewHeap::new()));

        let mut addressable = buffer.try_as_string_view_addressable_mut()?;

        for (idx, v) in iter.enumerate() {
            addressable.put(idx, v);
        }

        Ok(Array {
            datatype: DataType::Utf8,
            selection2: None,
            validity2: None,
            data2: ArrayData2::UntypedNull(UntypedNullStorage(len)),
            next: Some(ArrayNextInner {
                validity: Validity::new_all_valid(len),
                data: ArrayData::owned(buffer),
            }),
        })
    }
}

/// From iterator implementation that creates an array from optionally valid
/// values. Some is treated as valid, None as invalid.
impl<V> TryFromExactSizeIterator<Option<V>> for Array<NopBufferManager>
where
    V: Default,
    Array<NopBufferManager>: TryFromExactSizeIterator<V, Error = RayexecError>,
{
    type Error = RayexecError;

    fn try_from_iter<T: stdutil::iter::IntoExactSizeIterator<Item = Option<V>>>(
        iter: T,
    ) -> Result<Self, Self::Error> {
        let iter = iter.into_iter();
        let len = iter.len();

        let mut validity = Validity::new_all_valid(len);

        // New iterator that just uses the default value for missing values, and
        // sets the validity as appropriate.
        let iter = iter.enumerate().map(|(idx, v)| {
            if v.is_none() {
                validity.set_invalid(idx);
            }
            v.unwrap_or_default()
        });

        let mut array = Self::try_from_iter(iter)?;
        array.put_validity(validity)?;

        Ok(array)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn select_mut_no_change() {
        let mut arr = Array::from_iter(["a", "b", "c"]);
        let selection = SelectionVector::with_range(0..3);

        arr.select_mut2(selection);

        assert_eq!(ScalarValue::from("a"), arr.logical_value(0).unwrap());
        assert_eq!(ScalarValue::from("b"), arr.logical_value(1).unwrap());
        assert_eq!(ScalarValue::from("c"), arr.logical_value(2).unwrap());
    }

    #[test]
    fn select_mut_prune_rows() {
        let mut arr = Array::from_iter(["a", "b", "c"]);
        let selection = SelectionVector::from_iter([0, 2]);

        arr.select_mut2(selection);

        assert_eq!(ScalarValue::from("a"), arr.logical_value(0).unwrap());
        assert_eq!(ScalarValue::from("c"), arr.logical_value(1).unwrap());
        assert!(arr.logical_value(2).is_err());
    }

    #[test]
    fn select_mut_expand_rows() {
        let mut arr = Array::from_iter(["a", "b", "c"]);
        let selection = SelectionVector::from_iter([0, 1, 1, 2]);

        arr.select_mut2(selection);

        assert_eq!(ScalarValue::from("a"), arr.logical_value(0).unwrap());
        assert_eq!(ScalarValue::from("b"), arr.logical_value(1).unwrap());
        assert_eq!(ScalarValue::from("b"), arr.logical_value(2).unwrap());
        assert_eq!(ScalarValue::from("c"), arr.logical_value(3).unwrap());
        assert!(arr.logical_value(4).is_err());
    }

    #[test]
    fn select_mut_existing_selection() {
        let mut arr = Array::from_iter(["a", "b", "c"]);
        let selection = SelectionVector::from_iter([0, 2]);

        // => ["a", "c"]
        arr.select_mut2(selection);

        let selection = SelectionVector::from_iter([1, 1, 0]);
        arr.select_mut2(selection);

        assert_eq!(ScalarValue::from("c"), arr.logical_value(0).unwrap());
        assert_eq!(ScalarValue::from("c"), arr.logical_value(1).unwrap());
        assert_eq!(ScalarValue::from("a"), arr.logical_value(2).unwrap());
        assert!(arr.logical_value(3).is_err());
    }

    #[test]
    fn scalar_value_logical_eq_i32() {
        let arr = Array::from_iter([1, 2, 3]);
        let scalar = ScalarValue::Int32(2);

        assert!(!arr.scalar_value_logically_eq(&scalar, 0).unwrap());
        assert!(arr.scalar_value_logically_eq(&scalar, 1).unwrap());
    }

    #[test]
    fn scalar_value_logical_eq_null() {
        let arr = Array::from_iter([Some(1), None, Some(3)]);
        let scalar = ScalarValue::Null;

        assert!(!arr.scalar_value_logically_eq(&scalar, 0).unwrap());
        assert!(arr.scalar_value_logically_eq(&scalar, 1).unwrap());
    }
}
