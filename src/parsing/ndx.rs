use std::io::{self, BufRead};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NdxGroup {
    pub name: String,
    pub entries: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NdxFile {
    pub groups: Vec<NdxGroup>,
}

impl NdxFile {
    pub fn parse(input: &str) -> Result<Self, ParseNdxError> {
        let cursor = io::Cursor::new(input);
        Self::from_reader(cursor)
    }

    pub fn from_reader<R: BufRead>(reader: R) -> Result<Self, ParseNdxError> {
        let mut groups = Vec::new();
        let mut current_group: Option<NdxGroup> = None;

        for (line_no, line_result) in reader.lines().enumerate() {
            let line_no = line_no + 1;
            let line = line_result.map_err(ParseNdxError::Io)?;
            let line = line.trim();

            // 空行は無視
            if line.is_empty() {
                continue;
            }

            // グループ定義: [ group_name ]
            if line.starts_with('[') {
                if !line.ends_with(']') {
                    return Err(ParseNdxError::InvalidGroupHeader {
                        line: line_no,
                        content: line.to_string(),
                    });
                }

                // 既存グループを保存
                if let Some(group) = current_group.take() {
                    groups.push(group);
                }

                let name = line
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim()
                    .to_string();

                if name.is_empty() {
                    return Err(ParseNdxError::EmptyGroupName { line: line_no });
                }

                current_group = Some(NdxGroup {
                    name,
                    entries: Vec::new(),
                });

                continue;
            }

            // エントリ行
            let group = current_group
                .as_mut()
                .ok_or(ParseNdxError::EntryOutsideGroup { line: line_no })?;

            for token in line.split_whitespace() {
                let value = token
                    .parse::<u32>()
                    .map_err(|_| ParseNdxError::InvalidEntry {
                        line: line_no,
                        token: token.to_string(),
                    })?;

                group.entries.push(value);
            }
        }

        // 最後のグループを保存
        if let Some(group) = current_group {
            groups.push(group);
        }

        Ok(NdxFile { groups })
    }
}
#[derive(Debug)]
pub enum ParseNdxError {
    Io(io::Error),

    InvalidGroupHeader { line: usize, content: String },

    EmptyGroupName { line: usize },

    EntryOutsideGroup { line: usize },

    InvalidEntry { line: usize, token: String },
}

impl std::fmt::Display for ParseNdxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),

            Self::InvalidGroupHeader { line, content } => {
                write!(f, "invalid group header at line {line}: {content}")
            }

            Self::EmptyGroupName { line } => {
                write!(f, "empty group name at line {line}")
            }

            Self::EntryOutsideGroup { line } => {
                write!(f, "entry outside group at line {line}")
            }

            Self::InvalidEntry { line, token } => {
                write!(f, "invalid entry at line {line}: {token}")
            }
        }
    }
}

impl std::error::Error for ParseNdxError {}
