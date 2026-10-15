//! Nested values as statements spell them: a struct packed from named
//! fields, a list collected from rows, a list's items read back as rows, and
//! one item of a list, whose positions DataFusion numbers from 1.
use super::SqlLiteral;
use std::fmt::{self, Display, Formatter};

/// A struct a statement packs from named fields, each an expression:
/// `named_struct('start_ns', start_ns, 'end_ns', end_ns)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SqlStruct {
    fields: Vec<(String, String)>,
}

impl SqlStruct {
    pub fn new() -> Self {
        Self::default()
    }
    /// Each of `columns`, a field named after it.
    pub fn of_columns<'a>(columns: impl IntoIterator<Item = &'a str>) -> Self {
        columns
            .into_iter()
            .fold(Self::new(), |packed, column| packed.field(column, column))
    }
    /// The same, with field `name` holding `value`.
    pub fn field(mut self, name: &str, value: impl Into<String>) -> Self {
        self.fields.push((name.to_owned(), value.into()));
        self
    }
    /// A list holding this struct alone.
    pub fn listed(&self) -> String {
        format!("make_array({self})")
    }
    /// This struct of every row of a group, collected into one list in no
    /// particular order.
    pub fn collected(&self) -> String {
        format!("array_agg({self})")
    }
    /// This struct of every row of a group, collected into one list in
    /// `order`.
    pub fn collected_by(&self, order: &str) -> String {
        format!("array_agg({self} ORDER BY {order})")
    }
}

impl Display for SqlStruct {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let fields = self
            .fields
            .iter()
            .map(|(name, value)| format!("{}, {value}", SqlLiteral::text(name)))
            .collect::<Vec<_>>();
        write!(formatter, "named_struct({})", fields.join(", "))
    }
}

/// One row per item of a list each row of `source` holds, beside columns of
/// that row: `(SELECT r.episode_id, unnest(r.kept) AS k FROM source r)`. A row
/// whose list is empty or NULL has none. The relation is aliased by whoever
/// reads it, an item's field read as `<alias>.<item>.<field>`.
#[derive(Clone, Debug, PartialEq)]
pub struct ListItems {
    source: String,
    unnested: String,
    item: String,
    beside: Vec<String>,
}

impl ListItems {
    /// The items of `source`'s list column `list`, each named `item`.
    pub fn of(source: impl Display, list: &str, item: &str) -> Self {
        Self {
            source: source.to_string(),
            unnested: format!("r.{list}"),
            item: item.to_owned(),
            beside: Vec::new(),
        }
    }
    /// The positions of `source`'s list column `list`, from 0, each named
    /// `item`: what [`SqlList::item`] reads an item of the list at, when the
    /// row carries it beside.
    pub fn positions(source: impl Display, list: &str, item: &str) -> Self {
        Self {
            unnested: format!("range(cardinality(r.{list}))"),
            ..Self::of(source, list, item)
        }
    }
    /// The same, each item beside its row's `columns`.
    pub fn beside<'a>(mut self, columns: impl IntoIterator<Item = &'a str>) -> Self {
        self.beside.extend(columns.into_iter().map(str::to_owned));
        self
    }
}

impl Display for ListItems {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let beside = self
            .beside
            .iter()
            .map(|column| format!("r.{column}, "))
            .collect::<String>();
        write!(
            formatter,
            "(SELECT {beside}unnest({}) AS {} FROM {} r)",
            self.unnested, self.item, self.source
        )
    }
}

/// One item of a list, by its position from 0, as every caller counts:
/// DataFusion counts from 1.
pub struct SqlList;

impl SqlList {
    /// The item of `list` at the position the expression `position` holds.
    pub fn item(list: &str, position: &str) -> String {
        format!("array_element({list}, {position} + 1)")
    }
    /// The item of `list` at `position`.
    pub fn item_at(list: &str, position: usize) -> String {
        format!("array_element({list}, {})", position + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_struct_names_each_field_and_lists_or_collects_as_one_value() {
        let range = SqlStruct::of_columns(["start_ns", "end_ns"]);
        assert_eq!(
            range.to_string(),
            "named_struct('start_ns', start_ns, 'end_ns', end_ns)"
        );
        assert_eq!(
            range.listed(),
            "make_array(named_struct('start_ns', start_ns, 'end_ns', end_ns))"
        );
        assert_eq!(
            range.collected_by("start_ns"),
            "array_agg(named_struct('start_ns', start_ns, 'end_ns', end_ns) ORDER BY start_ns)"
        );
        let departure = SqlStruct::new()
            .field("episode_id", "d.episode_id")
            .field("it's", "d.hz");
        assert_eq!(
            departure.collected(),
            "array_agg(named_struct('episode_id', d.episode_id, 'it''s', d.hz))"
        );
    }

    #[test]
    fn a_lists_items_and_positions_read_as_rows_beside_their_row() {
        assert_eq!(
            ListItems::of("episodes", "kept", "k")
                .beside(["episode_id"])
                .to_string(),
            "(SELECT r.episode_id, unnest(r.kept) AS k FROM episodes r)"
        );
        assert_eq!(
            ListItems::of("(SELECT streams FROM e)", "streams", "s").to_string(),
            "(SELECT unnest(r.streams) AS s FROM (SELECT streams FROM e) r)"
        );
        assert_eq!(
            ListItems::positions("episodes", "subtasks", "i")
                .beside(["episode_id", "subtasks"])
                .to_string(),
            "(SELECT r.episode_id, r.subtasks, unnest(range(cardinality(r.subtasks))) AS i FROM episodes r)"
        );
        assert_eq!(
            SqlList::item("g.subtasks", "g.i"),
            "array_element(g.subtasks, g.i + 1)"
        );
        assert_eq!(SqlList::item_at("tasks", 0), "array_element(tasks, 1)");
    }
}
