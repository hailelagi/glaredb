use std::marker::PhantomData;
use std::ops::AddAssign;

use num_traits::{AsPrimitive, Float};
use rayexec_error::Result;

use crate::arrays::array::physical_type::{PhysicalF16, PhysicalF32, PhysicalF64, PhysicalStorage};
use crate::arrays::array::Array;
use crate::arrays::datatype::{DataType, DataTypeId};
use crate::arrays::executor::builder::{ArrayBuilder, PrimitiveBuffer};
use crate::arrays::executor::scalar::{BinaryListReducer, ListExecutor};
use crate::expr::Expression;
use crate::functions::documentation::{Category, Documentation, Example};
use crate::functions::scalar::{PlannedScalarFunction, ScalarFunction, ScalarFunctionImpl};
use crate::functions::{invalid_input_types_error, plan_check_num_args, FunctionInfo, Signature};
use crate::logical::binder::table_list::TableList;

/// Euclidean distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L2Distance;

impl FunctionInfo for L2Distance {
    fn name(&self) -> &'static str {
        "l2_distance"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["array_distance"]
    }

    fn signatures(&self) -> &[Signature] {
        // TODO: Ideally return type would depend on the primitive type in the
        // list.
        &[Signature {
            positional_args: &[DataTypeId::List, DataTypeId::List],
            variadic_arg: None,
            return_type: DataTypeId::Float64,
            doc: Some(&Documentation{
                category: Category::List,
                description: "Compute the Euclidean distance between two lists. Both lists must be the same length and cannot contain NULLs.",
                arguments: &["list1", "list2"],
                example: Some(Example{
                    example: "l2_distance([1.0, 1.0], [2.0, 4.0])",
                    output: "3.1622776601683795",
                }),
            }),
        }]
    }
}

impl ScalarFunction for L2Distance {
    fn plan(
        &self,
        table_list: &TableList,
        inputs: Vec<Expression>,
    ) -> Result<PlannedScalarFunction> {
        plan_check_num_args(self, &inputs, 2)?;

        let function_impl: Box<dyn ScalarFunctionImpl> = match (
            inputs[0].datatype(table_list)?,
            inputs[1].datatype(table_list)?,
        ) {
            (DataType::List(a), DataType::List(b)) => {
                match (a.datatype.as_ref(), b.datatype.as_ref()) {
                    (DataType::Float16, DataType::Float16) => {
                        Box::new(L2DistanceImpl::<PhysicalF16>::new())
                    }
                    (DataType::Float32, DataType::Float32) => {
                        Box::new(L2DistanceImpl::<PhysicalF32>::new())
                    }
                    (DataType::Float64, DataType::Float64) => {
                        Box::new(L2DistanceImpl::<PhysicalF64>::new())
                    }
                    (a, b) => return Err(invalid_input_types_error(self, &[a, b])),
                }
            }
            (a, b) => return Err(invalid_input_types_error(self, &[a, b])),
        };

        Ok(PlannedScalarFunction {
            function: Box::new(*self),
            return_type: DataType::Float64,
            inputs,
            function_impl,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L2DistanceImpl<S: PhysicalStorage> {
    _s: PhantomData<S>,
}

impl<S> L2DistanceImpl<S>
where
    S: PhysicalStorage,
{
    fn new() -> Self {
        L2DistanceImpl { _s: PhantomData }
    }
}

impl<S> ScalarFunctionImpl for L2DistanceImpl<S>
where
    S: PhysicalStorage,
    for<'a> S::Type<'a>: Float + AddAssign + AsPrimitive<f64> + Default + Copy,
{
    fn execute(&self, inputs: &[&Array]) -> Result<Array> {
        let a = inputs[0];
        let b = inputs[1];

        let builder = ArrayBuilder {
            datatype: DataType::Float64,
            buffer: PrimitiveBuffer::with_len(a.logical_len()),
        };

        ListExecutor::<false, false>::binary_reduce::<S, _, L2DistanceReducer<_>>(a, b, builder)
    }
}

#[derive(Debug, Default)]
pub(crate) struct L2DistanceReducer<F> {
    pub distance: F,
}

impl<F> BinaryListReducer<F, f64> for L2DistanceReducer<F>
where
    F: Float + AddAssign + AsPrimitive<f64> + Default,
{
    fn new(left_len: i32, right_len: i32) -> Self {
        debug_assert_eq!(left_len, right_len);
        Self::default()
    }

    fn put_values(&mut self, v1: F, v2: F) {
        let diff = v1 - v2;
        self.distance += diff * diff;
    }

    fn finish(self) -> f64 {
        self.distance.as_().sqrt()
    }
}
