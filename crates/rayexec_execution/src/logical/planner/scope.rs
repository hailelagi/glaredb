use std::collections::HashSet;
use std::fmt;
use std::hash::Hash;

use rayexec_error::{RayexecError, Result};

/// Reference to a column in some scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ColumnRef {
    /// Scope level for where this column exists.
    ///
    /// 0 indicates "this" scope, 1 indicates a scope one level up, 2 indicates
    /// two levels ups, etc...
    pub scope_level: usize,

    /// Index of the item in the scope.
    pub item_idx: usize,
}

impl ColumnRef {
    /// Try to get the uncorrelated column index.
    ///
    /// An uncorrelated column index is a column index that exists in "this"
    /// scope (scope_level == 0).
    pub fn try_as_uncorrelated(&self) -> Result<usize> {
        if self.scope_level != 0 {
            return Err(RayexecError::new(format!(
                "Column is not uncorrelated: {self:?}"
            )));
        }
        Ok(self.item_idx)
    }
}

/// Reference to a table inside a scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableReference {
    pub database: Option<String>,
    pub schema: Option<String>,
    pub table: String,
}

impl TableReference {
    fn matches(&self, other: &TableReference) -> bool {
        match (&self.database, &other.database) {
            (Some(a), Some(b)) if a != b => return false,
            _ => (),
        }
        match (&self.schema, &other.schema) {
            (Some(a), Some(b)) if a != b => return false,
            _ => (),
        }

        self.table == other.table
    }
}

impl fmt::Display for TableReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(database) = &self.database {
            write!(f, "{database}")?;
        }
        if let Some(schema) = &self.schema {
            write!(f, "{schema}")?;
        }
        write!(f, "{}", self.table)
    }
}

/// An item in a scope with an optional alias.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeColumn {
    /// Alias of the table containing this column, either user provided or the
    /// table name itself.
    ///
    /// May be None in the case of a column being added through the use of a
    /// USING join constraint. In which case, the only way to reference the item
    /// in the scope is through its unqualified name.
    pub alias: Option<TableReference>,

    /// Name of the column.
    pub column: String,
}

/// Provides a scope items introduced in the FROM clause of a query.
#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    /// Items in scope.
    pub items: Vec<ScopeColumn>,
}

impl Scope {
    /// Create a new empty scope.
    pub const fn empty() -> Self {
        Scope { items: Vec::new() }
    }

    /// Create a new scope with the given columns for a table.
    pub fn with_columns<S, I>(alias: Option<TableReference>, columns: I) -> Self
    where
        S: Into<String>,
        I: IntoIterator<Item = S>,
    {
        let mut scope = Scope::empty();
        scope.add_columns(alias, columns);
        scope
    }

    /// Add columns to this scope for a table with the given alias.
    pub fn add_columns<S, I>(&mut self, alias: Option<TableReference>, columns: I)
    where
        S: Into<String>,
        I: IntoIterator<Item = S>,
    {
        let iter = columns.into_iter().map(|column| ScopeColumn {
            alias: alias.clone(),
            column: column.into(),
        });
        self.items.extend(iter);
    }

    /// Try to resolve a column.
    ///
    /// First searches this scope, then attempts to find the column in an outer
    /// scope.
    ///
    /// Outer scopes are search left to right, with the left-most scope
    /// representing the inner-most scope.
    ///
    /// Returns an error if the column is ambigious. Returns None if there
    /// exists no columns.
    pub fn resolve_column(
        &self,
        outer: &[Scope],
        table: Option<&TableReference>,
        column: &str,
    ) -> Result<Option<ColumnRef>> {
        if let Some(idx) = self.column_index(table, column)? {
            // Column found in this scope.
            return Ok(Some(ColumnRef {
                scope_level: 0,
                item_idx: idx,
            }));
        }

        // Search outer scopes.
        for (scope_level, scope) in outer.iter().enumerate() {
            if let Some(idx) = scope.column_index(table, column)? {
                // Column found in outer scope.
                return Ok(Some(ColumnRef {
                    scope_level: scope_level + 1,
                    item_idx: idx,
                }));
            }
        }

        // No columns found.
        Ok(None)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ScopeColumn> {
        self.items.iter_mut()
    }

    /// Find the index of a column with the given name.
    ///
    /// Errors if multiple columns with the same name are found.
    fn column_index(&self, alias: Option<&TableReference>, column: &str) -> Result<Option<usize>> {
        let pred = |item: &ScopeColumn| match (alias, &item.alias) {
            (Some(alias), Some(item_alias)) => alias.matches(item_alias) && item.column == column,
            (Some(_), None) => false,
            (None, _) => item.column == column,
        };

        let mut iter = self.items.iter();
        let idx = match iter.position(pred) {
            Some(idx) => idx,
            None => return Ok(None),
        };

        // Check to make sure there's no other columns with this name in the
        // scope.
        if iter.any(pred) {
            return Err(RayexecError::new(format!("Ambiguous column name {column}")));
        }

        Ok(Some(idx))
    }

    /// Merge another scope into this one.
    ///
    /// Errors on duplicate table aliases.
    pub fn merge(mut self, mut right: Scope) -> Result<Self> {
        let left_aliases: HashSet<_> = self.table_aliases_iter().collect();
        for alias in right.table_aliases_iter() {
            if left_aliases.contains(alias) {
                return Err(RayexecError::new(format!("Duplicate table name: {alias}")));
            }
        }

        self.items.append(&mut right.items);

        Ok(self)
    }

    pub fn num_columns(&self) -> usize {
        self.items.len()
    }

    pub fn column_name_iter(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(|item| item.column.as_str())
    }

    fn table_aliases_iter(&self) -> impl Iterator<Item = &TableReference> {
        self.items.iter().filter_map(|item| item.alias.as_ref())
    }
}
