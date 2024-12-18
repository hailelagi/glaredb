use std::fmt;

use rayexec_bullet::batch::Batch;
use rayexec_bullet::field::Schema;
use rayexec_bullet::format::{FormatOptions, Formatter};
use rayexec_error::Result;

const FORMATTER: Formatter = Formatter::new(FormatOptions {
    null: "",
    empty_string: "",
});

pub fn write_markdown_table<'a>(
    output: &mut dyn fmt::Write,
    schema: &Schema,
    batches: impl IntoIterator<Item = &'a Batch>,
) -> Result<()> {
    // 'field1 | field2 | field3'
    let header = schema
        .fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<Vec<_>>()
        .join(" | ");

    writeln!(output, "| {header} |")?;

    // ' --- | --- | ---'
    let sep = schema
        .fields
        .iter()
        .map(|_| "---")
        .collect::<Vec<_>>()
        .join(" | ");

    writeln!(output, "| {sep} |")?;

    for batch in batches {
        for row in 0..batch.num_rows() {
            for (idx, column) in batch.columns().iter().enumerate() {
                if idx == 0 {
                    write!(output, "|")?;
                }

                let val = FORMATTER.format_array_value(column, row)?;
                write!(output, " {val} |")?;
            }
            writeln!(output)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use rayexec_bullet::array::Array;
    use rayexec_bullet::datatype::DataType;
    use rayexec_bullet::field::Field;

    use super::*;

    #[test]
    fn simple() {
        let batch = Batch::try_new([
            Array::from_iter([1, 2, 3]),
            Array::from_iter(["cat", "dog", "mouse"]),
        ])
        .unwrap();

        let schema = Schema::new([
            Field::new("Numbers", DataType::Int32, false),
            Field::new("Strings", DataType::Utf8, false),
        ]);

        let mut buf = String::new();

        write_markdown_table(&mut buf, &schema, [&batch]).unwrap();

        let expected = r#"| Numbers | Strings |
| --- | --- |
| 1 | cat |
| 2 | dog |
| 3 | mouse |
"#;

        assert_eq!(expected, buf);
    }
}