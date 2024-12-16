use rayexec_bullet::array::Array;
use rayexec_bullet::datatype::{DataType, DataTypeId};
use rayexec_bullet::executor::builder::{ArrayBuilder, PrimitiveBuffer};
use rayexec_bullet::executor::physical_type::{PhysicalBinary, PhysicalUtf8};
use rayexec_bullet::executor::scalar::UnaryExecutor;
use rayexec_error::Result;

use crate::expr::Expression;
use crate::functions::scalar::{PlannedScalarFunction, ScalarFunction, ScalarFunctionImpl};
use crate::functions::{invalid_input_types_error, plan_check_num_args, FunctionInfo, Signature};
use crate::logical::binder::table_list::TableList;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Length;

impl FunctionInfo for Length {
    fn name(&self) -> &'static str {
        "length"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["char_length", "character_length"]
    }

    fn signatures(&self) -> &[Signature] {
        &[Signature {
            positional_args: &[DataTypeId::Utf8],
            variadic_arg: None,
            return_type: DataTypeId::Int64,
        }]
    }
}

impl ScalarFunction for Length {
    fn plan(
        &self,
        table_list: &TableList,
        inputs: Vec<Expression>,
    ) -> Result<PlannedScalarFunction> {
        plan_check_num_args(self, &inputs, 1)?;
        match inputs[0].datatype(table_list)? {
            DataType::Utf8 => Ok(PlannedScalarFunction {
                function: Box::new(*self),
                return_type: DataType::Int64,
                inputs,
                function_impl: Box::new(StrLengthImpl),
            }),
            a => Err(invalid_input_types_error(self, &[a])),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrLengthImpl;

impl ScalarFunctionImpl for StrLengthImpl {
    fn execute(&self, inputs: &[&Array]) -> Result<Array> {
        let input = inputs[0];

        let builder = ArrayBuilder {
            datatype: DataType::Int64,
            buffer: PrimitiveBuffer::with_len(input.logical_len()),
        };

        UnaryExecutor::execute::<PhysicalUtf8, _, _>(input, builder, |v, buf| {
            let len = v.chars().count() as i64;
            buf.put(&len)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteLength;

impl FunctionInfo for ByteLength {
    fn name(&self) -> &'static str {
        "byte_length"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["octet_length"]
    }

    fn signatures(&self) -> &[Signature] {
        &[
            Signature {
                positional_args: &[DataTypeId::Utf8],
                variadic_arg: None,
                return_type: DataTypeId::Int64,
            },
            Signature {
                positional_args: &[DataTypeId::Binary],
                variadic_arg: None,
                return_type: DataTypeId::Int64,
            },
        ]
    }
}

impl ScalarFunction for ByteLength {
    fn plan(
        &self,
        table_list: &TableList,
        inputs: Vec<Expression>,
    ) -> Result<PlannedScalarFunction> {
        plan_check_num_args(self, &inputs, 1)?;
        match inputs[0].datatype(table_list)? {
            DataType::Utf8 | DataType::Binary => Ok(PlannedScalarFunction {
                function: Box::new(*self),
                return_type: DataType::Int64,
                inputs,
                function_impl: Box::new(ByteLengthImpl),
            }),
            a => Err(invalid_input_types_error(self, &[a])),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteLengthImpl;

impl ScalarFunctionImpl for ByteLengthImpl {
    fn execute(&self, inputs: &[&Array]) -> Result<Array> {
        let input = inputs[0];

        let builder = ArrayBuilder {
            datatype: DataType::Int64,
            buffer: PrimitiveBuffer::with_len(input.logical_len()),
        };

        // Binary applicable to both str and [u8].
        UnaryExecutor::execute::<PhysicalBinary, _, _>(input, builder, |v, buf| {
            buf.put(&(v.len() as i64))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitLength;

impl FunctionInfo for BitLength {
    fn name(&self) -> &'static str {
        "bit_length"
    }

    fn signatures(&self) -> &[Signature] {
        &[
            Signature {
                positional_args: &[DataTypeId::Utf8],
                variadic_arg: None,
                return_type: DataTypeId::Int64,
            },
            Signature {
                positional_args: &[DataTypeId::Binary],
                variadic_arg: None,
                return_type: DataTypeId::Int64,
            },
        ]
    }
}

impl ScalarFunction for BitLength {
    fn plan(
        &self,
        table_list: &TableList,
        inputs: Vec<Expression>,
    ) -> Result<PlannedScalarFunction> {
        plan_check_num_args(self, &inputs, 1)?;
        match inputs[0].datatype(table_list)? {
            DataType::Utf8 | DataType::Binary => Ok(PlannedScalarFunction {
                function: Box::new(*self),
                return_type: DataType::Int64,
                inputs,
                function_impl: Box::new(BitLengthImpl),
            }),
            a => Err(invalid_input_types_error(self, &[a])),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitLengthImpl;

impl ScalarFunctionImpl for BitLengthImpl {
    fn execute(&self, inputs: &[&Array]) -> Result<Array> {
        let input = inputs[0];

        let builder = ArrayBuilder {
            datatype: DataType::Int64,
            buffer: PrimitiveBuffer::with_len(input.logical_len()),
        };

        // Binary applicable to both str and [u8].
        UnaryExecutor::execute::<PhysicalBinary, _, _>(input, builder, |v, buf| {
            let bit_len = v.len() * 8;
            buf.put(&(bit_len as i64))
        })
    }
}