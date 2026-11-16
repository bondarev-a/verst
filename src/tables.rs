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
                    if let Some(mut table) = build.finish(source) {
                        refine_from_pipes(source, &mut table);
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
    let count = parse_tables(source).get(table)?.rows.len();
    insert_row_at(source, table, count)
}

/// `at` — индекс среди строк данных (0 сразу под заголовком).
pub fn insert_row_at(source: &str, table: usize, at: usize) -> Option<String> {
    splice_table(source, table, |item| {
        let cols = item.column_count();
        let mut rows = row_strings(item);
        let at = at.min(rows.len());
        rows.insert(at, vec!["  ".to_string(); cols]);
        Some(format_md_table(&header_strings(item), &item.alignments, &rows))
    })
}

/// `row` — индекс в сетке: 0 заголовок, дальше данные. Заголовок не удаляется.
pub fn delete_row_at(source: &str, table: usize, row: usize) -> Option<String> {
    if row == 0 {
        return None;
    }
    splice_table(source, table, |item| {
        let mut rows = row_strings(item);
        let index = row - 1;
        if index >= rows.len() {
            return None;
        }
        rows.remove(index);
        Some(format_md_table(&header_strings(item), &item.alignments, &rows))
    })
}

/// Добавить колонку во все строки, включая строку выравнивания.
pub fn add_column(source: &str, table: usize) -> Option<String> {
    let cols = parse_tables(source).get(table)?.column_count();
    insert_column_at(source, table, cols)
}

/// Вставить пустую колонку в позиции `col` (текущая уезжает вправо).
pub fn insert_column_at(source: &str, table: usize, col: usize) -> Option<String> {
    splice_table(source, table, |item| {
        let mut header = header_strings(item);
        let mut rows = row_strings(item);
        let mut alignments = item.alignments.clone();
        let at = col.min(header.len());
        header.insert(at, "  ".to_string());
        alignments.insert(at, ColAlign::None);
        for row in &mut rows {
            let at = at.min(row.len());
            row.insert(at, "  ".to_string());
        }
        Some(format_md_table(&header, &alignments, &rows))
    })
}

pub fn delete_column_at(source: &str, table: usize, col: usize) -> Option<String> {
    splice_table(source, table, |item| {
        if item.column_count() <= 1 || col >= item.column_count() {
            return None;
        }
        let mut header = header_strings(item);
        let mut rows = row_strings(item);
        let mut alignments = item.alignments.clone();
        header.remove(col);
        if col < alignments.len() {
            alignments.remove(col);
        }
        for row in &mut rows {
            if col < row.len() {
                row.remove(col);
            }
        }
        Some(format_md_table(&header, &alignments, &rows))
    })
}

fn splice_table(source: &str, table: usize, edit: impl FnOnce(&MdTable) -> Option<String>) -> Option<String> {
    let tables = parse_tables(source);
    let item = tables.get(table)?;
    let rebuilt = edit(item)?;
    let mut out = String::with_capacity(source.len() + rebuilt.len());
    out.push_str(&source[..item.start]);
    out.push_str(&rebuilt);
    out.push_str(&source[item.end..]);
    Some(out)
}

fn header_strings(table: &MdTable) -> Vec<String> {
    table.header.iter().map(|cell| cell.text.clone()).collect()
}

fn row_strings(table: &MdTable) -> Vec<Vec<String>> {
    table
        .rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.text.clone()).collect())
        .collect()
}

fn pad_row(mut row: Vec<String>, cols: usize) -> Vec<String> {
    while row.len() < cols {
        row.push("  ".to_string());
    }
    row.truncate(cols);
    row
}

fn write_row(cells: &[String]) -> String {
    let mut line = String::from("|");
    for cell in cells {
        line.push_str(cell);
        line.push('|');
    }
    line.push('\n');
    line
}

fn write_align(alignments: &[ColAlign]) -> String {
    let mut line = String::from("|");
    for align in alignments {
        line.push_str(match align {
            ColAlign::None => " --- ",
            ColAlign::Left => " :--- ",
            ColAlign::Center => " :---: ",
            ColAlign::Right => " ---: ",
        });
        line.push('|');
    }
    line.push('\n');
    line
}

fn format_md_table(header: &[String], alignments: &[ColAlign], rows: &[Vec<String>]) -> String {
    let cols = header.len().max(1);
    let header = pad_row(header.to_vec(), cols);
    let mut alignments = alignments.to_vec();
    alignments.resize(cols, ColAlign::None);
    let mut out = write_row(&header);
    out.push_str(&write_align(&alignments));
    for row in rows {
        out.push_str(&write_row(&pad_row(row.clone(), cols)));
    }
    out
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

fn is_align_line(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains("---") {
        return false;
    }
    trimmed.chars().all(|ch| matches!(ch, '|' | ':' | '-' | ' ' | '\t'))
}

fn pipe_row_cells(source: &str, line_start: usize, line: &str) -> Vec<TableCell> {
    let line = line.trim_end_matches('\n').trim_end_matches('\r');
    if !line.contains('|') {
        return Vec::new();
    }
    let mut cells = Vec::new();
    let mut i = if line.starts_with('|') { 1 } else { 0 };
    let mut cell_at = line_start + i;
    while i < line.len() {
        let ch = line[i..].chars().next().unwrap();
        if ch == '|' {
            cells.push(TableCell {
                start: cell_at,
                end: line_start + i,
                text: source[cell_at..line_start + i].to_string(),
            });
            i += 1;
            cell_at = line_start + i;
        } else {
            i += ch.len_utf8();
        }
    }
    cells
}

/// pulldown-cmark режет колонки по строке `---`. Число `|` в строках может быть больше.
fn refine_from_pipes(source: &str, table: &mut MdTable) {
    let end = table.end.min(source.len());
    let mut pos = table.start.min(end);
    let mut header = Vec::new();
    let mut rows = Vec::new();
    let mut seen_align = false;
    while pos < end {
        let rest = &source[pos..end];
        let width = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
        let line = &source[pos..pos + width];
        if is_align_line(line) {
            seen_align = true;
            pos += width;
            continue;
        }
        let cells = pipe_row_cells(source, pos, line);
        if !cells.is_empty() {
            if !seen_align && header.is_empty() {
                header = cells;
            } else {
                rows.push(cells);
            }
        }
        pos += width;
    }
    if header.is_empty() {
        return;
    }
    let cols = header.len();
    table.header = header;
    table.rows = rows;
    if table.alignments.len() < cols {
        table.alignments.resize(cols, ColAlign::None);
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
    fn extra_header_pipes_keep_all_columns() {
        let source = "| № | Роль | Фон | Паттерн |\n| --- | --- | --- | --- |\n| 1 | a | b | c |\n";
        let tables = parse_tables(source);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].column_count(), 4);
        assert_eq!(tables[0].header[3].text.trim(), "Паттерн");
        assert_eq!(tables[0].rows[0][3].text.trim(), "c");
        assert_eq!(&source[tables[0].start..tables[0].end], source);
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
    fn insert_row_and_column_around_cell() {
        let source = sample();
        let above = insert_row_at(source, 0, 0).unwrap();
        let tables = parse_tables(&above);
        assert_eq!(tables[0].rows.len(), 3);
        assert_eq!(tables[0].rows[0][0].text.trim(), "");
        assert_eq!(tables[0].rows[1][0].text.trim(), "apples");
        let left = insert_column_at(source, 0, 0).unwrap();
        let tables = parse_tables(&left);
        assert_eq!(tables[0].column_count(), 3);
        assert_eq!(tables[0].header[1].text.trim(), "Name");
        let right = insert_column_at(source, 0, 1).unwrap();
        let tables = parse_tables(&right);
        assert_eq!(tables[0].header[1].text.trim(), "");
        assert_eq!(tables[0].header[2].text.trim(), "Qty");
    }

    #[test]
    fn delete_row_and_column_keep_header() {
        let source = sample();
        assert!(delete_row_at(source, 0, 0).is_none());
        let gone = delete_row_at(source, 0, 1).unwrap();
        let tables = parse_tables(&gone);
        assert_eq!(tables[0].rows.len(), 1);
        assert_eq!(tables[0].rows[0][0].text.trim(), "**tea**");
        let one_col = delete_column_at(source, 0, 1).unwrap();
        let tables = parse_tables(&one_col);
        assert_eq!(tables[0].column_count(), 1);
        assert!(delete_column_at(&one_col, 0, 0).is_none());
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
