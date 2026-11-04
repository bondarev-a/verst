use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Выравнивание колонки из строки `| --- | :---: | ---: |`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColAlign {
    None,
    Left,
    Center,
    Right,
}

/// Ячейка в исходном тексте: байтовый диапазон и сырой фрагмент между `|`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCell {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Распознанная Markdown-таблица. Строка выравнивания в сетку не входит.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdTable {
    pub start: usize,
    pub end: usize,
    pub align_start: usize,
    pub align_end: usize,
    pub alignments: Vec<ColAlign>,
    pub header: Vec<TableCell>,
    pub rows: Vec<Vec<TableCell>>,
}

impl MdTable {
    pub fn column_count(&self) -> usize {
        self.header.len()
    }

    /// Строка 0 — заголовок, дальше строки данных.
    pub fn cell(&self, row: usize, col: usize) -> Option<&TableCell> {
        if row == 0 {
            self.header.get(col)
        } else {
            self.rows.get(row - 1)?.get(col)
        }
    }

    pub fn row_count(&self) -> usize {
        1 + self.rows.len()
    }
}

/// Разбор таблиц по исходному тексту (без переписывания вики-ссылок).
pub fn parse_tables(source: &str) -> Vec<MdTable> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(source, options).into_offset_iter();

    let mut tables = Vec::new();
    let mut current: Option<Building> = None;

    for (event, range) in parser {
        match event {
            Event::Start(Tag::Table(alignments)) => {
                current = Some(Building {
                    start: range.start,
                    end: range.end,
                    alignments: alignments.iter().copied().map(map_align).collect(),
                    header: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                    cell_start: None,
                    cell_end: None,
                    row_cells: Vec::new(),
                    head_end: None,
                    first_body_start: None,
                });
            }
            Event::Start(Tag::TableHead) => {
                if let Some(build) = current.as_mut() {
                    build.in_head = true;
                    build.row_cells.clear();
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(build) = current.as_mut() {
                    build.in_head = false;
                    build.header = std::mem::take(&mut build.row_cells);
                    build.head_end = Some(range.end);
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(build) = current.as_mut() {
                    build.row_cells.clear();
                    if build.first_body_start.is_none() {
                        build.first_body_start = Some(range.start);
                    }
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(build) = current.as_mut() {
                    if !build.in_head {
                        let cells = std::mem::take(&mut build.row_cells);
                        build.rows.push(cells);
                    }
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(build) = current.as_mut() {
                    build.cell_start = Some(range.start);
                    build.cell_end = Some(range.end);
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(build) = current.as_mut() {
                    let start = build.cell_start.take().unwrap_or(range.start);
                    let end = build
                        .cell_end
                        .take()
                        .unwrap_or(range.end)
                        .max(range.end)
                        .min(source.len())
                        .max(start);
                    let text = source[start..end].to_string();
                    build.row_cells.push(TableCell { start, end, text });
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(mut build) = current.take() {
                    build.end = range.end;
                    if let Some(table) = build.finish(source) {
                        tables.push(table);
                    }
                }
            }
            _ => {}
        }
    }
    tables
}

/// Заменить текст ячейки (`row` 0 — заголовок). Один кусок в файле.
pub fn replace_cell(source: &str, table: usize, row: usize, col: usize, text: &str) -> Option<String> {
    let tables = parse_tables(source);
    let cell = tables.get(table)?.cell(row, col)?;
    let mut out = String::with_capacity(source.len() + text.len());
    out.push_str(&source[..cell.start]);
    out.push_str(text);
    out.push_str(&source[cell.end..]);
    Some(out)
}

/// Добавить пустую строку данных в конец таблицы.
pub fn add_row(source: &str, table: usize) -> Option<String> {
    let tables = parse_tables(source);
    let table = tables.get(table)?;
    let line = empty_row(table.column_count());
    let mut out = String::with_capacity(source.len() + line.len());
    out.push_str(&source[..table.end]);
    out.push_str(&line);
    out.push_str(&source[table.end..]);
    Some(out)
}

/// Добавить колонку во все строки, включая строку выравнивания.
pub fn add_column(source: &str, table: usize) -> Option<String> {
    let tables = parse_tables(source);
    let table = tables.get(table)?;
    let rebuilt = format_table_with_extra_column(source, table);
    let mut out = String::with_capacity(source.len() + 16);
    out.push_str(&source[..table.start]);
    out.push_str(&rebuilt);
    out.push_str(&source[table.end..]);
    Some(out)
}

fn empty_row(cols: usize) -> String {
    let mut line = String::new();
    for _ in 0..cols {
        line.push_str("|  ");
    }
    line.push_str("|\n");
    line
}

fn format_table_with_extra_column(source: &str, table: &MdTable) -> String {
    let mut out = String::new();
    out.push_str(&row_with_extra(
        &table.header.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
        "  ",
    ));
    let align_line = source[table.align_start..table.align_end].trim_end_matches('\n');
    out.push_str(align_line);
    if !align_line.ends_with('|') {
        out.push('|');
    }
    out.push_str(" --- |\n");
    for row in &table.rows {
        out.push_str(&row_with_extra(
            &row.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            "  ",
        ));
    }
    out
}

fn row_with_extra(cells: &[&str], extra: &str) -> String {
    let mut line = String::new();
    line.push('|');
    for cell in cells {
        line.push_str(cell);
        line.push('|');
    }
    line.push_str(extra);
    line.push_str("|\n");
    line
}

fn map_align(value: pulldown_cmark::Alignment) -> ColAlign {
    match value {
        pulldown_cmark::Alignment::None => ColAlign::None,
        pulldown_cmark::Alignment::Left => ColAlign::Left,
        pulldown_cmark::Alignment::Center => ColAlign::Center,
        pulldown_cmark::Alignment::Right => ColAlign::Right,
    }
}

struct Building {
    start: usize,
    end: usize,
    alignments: Vec<ColAlign>,
    header: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
    in_head: bool,
    cell_start: Option<usize>,
    cell_end: Option<usize>,
    row_cells: Vec<TableCell>,
    head_end: Option<usize>,
    first_body_start: Option<usize>,
}

impl Building {
    fn finish(self, source: &str) -> Option<MdTable> {
        if self.header.is_empty() {
            return None;
        }
        let head_end = self.head_end.unwrap_or(self.start);
        let body_start = self.first_body_start.unwrap_or(self.end);
        let align_start = head_end;
        let align_end = body_start;
        if align_start > align_end || align_end > source.len() {
            return None;
        }
        Some(MdTable {
            start: self.start,
            end: self.end.min(source.len()),
            align_start,
            align_end,
            alignments: self.alignments,
            header: self.header,
            rows: self.rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        "| Name | Qty |\n| --- | ---: |\n| apples | 3 |\n| **tea** | 1 |\n"
    }

    #[test]
    fn parse_reads_bounds_cells_and_alignment() {
        let tables = parse_tables(sample());
        assert_eq!(tables.len(), 1);
        let table = &tables[0];
        assert_eq!(table.start, 0);
        assert_eq!(&sample()[table.start..table.end], sample());
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.header[0].text.trim(), "Name");
        assert_eq!(table.header[1].text.trim(), "Qty");
        assert_eq!(table.alignments, vec![ColAlign::None, ColAlign::Right]);
        assert!(sample()[table.align_start..table.align_end].contains("---"));
        assert!(!sample()[table.align_start..table.align_end].contains("apples"));
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0][0].text.trim(), "apples");
        assert_eq!(table.rows[1][0].text.trim(), "**tea**");
        assert_eq!(
            sample()[table.rows[1][0].start..table.rows[1][0].end].trim(),
            "**tea**"
        );
    }

    #[test]
    fn incomplete_pipe_line_stays_text() {
        let source = "Not a table\n| bad |\n";
        assert!(parse_tables(source).is_empty());
    }

    #[test]
    fn replace_cell_rewrites_only_that_span() {
        let source = sample();
        let next = replace_cell(source, 0, 1, 0, " pears ").unwrap();
        assert!(next.contains("| pears | 3 |"));
        assert!(next.contains("| **tea** | 1 |"));
        assert!(next.contains("| Name | Qty |"));
        assert!(!next.contains("apples"));
        let tables = parse_tables(&next);
        assert_eq!(tables[0].rows[0][0].text.trim(), "pears");
    }

    #[test]
    fn replace_header_cell() {
        let next = replace_cell(sample(), 0, 0, 1, " Count ").unwrap();
        assert!(next.starts_with("| Name | Count |\n"));
    }

    #[test]
    fn add_row_appends_empty_data_line() {
        let next = add_row(sample(), 0).unwrap();
        let tables = parse_tables(&next);
        assert_eq!(tables[0].rows.len(), 3);
        assert_eq!(tables[0].rows[2][0].text.trim(), "");
        assert_eq!(tables[0].rows[2][1].text.trim(), "");
        assert!(next.ends_with("|  |  |\n") || next.contains("|  |  |\n"));
    }

    #[test]
    fn add_column_extends_header_align_and_body() {
        let next = add_column(sample(), 0).unwrap();
        let tables = parse_tables(&next);
        assert_eq!(tables[0].column_count(), 3);
        assert_eq!(tables[0].header.len(), 3);
        assert_eq!(tables[0].rows[0].len(), 3);
        let align = &next[tables[0].align_start..tables[0].align_end];
        assert!(align.matches("---").count() >= 3);
    }

    #[test]
    fn positions_survive_wikilink_like_text() {
        let source = "| [[note]] | x |\n| --- | --- |\n| a | b |\n";
        let tables = parse_tables(source);
        let cell = &tables[0].header[0];
        assert_eq!(source[cell.start..cell.end].trim(), "[[note]]");
        let next = replace_cell(source, 0, 0, 0, " [[other]] ").unwrap();
        assert!(next.contains("[[other]]"));
        assert!(!next.contains("[[note]]"));
    }
}
