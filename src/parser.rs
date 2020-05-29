use crate::tokens::*;
use std::error::Error;
use std::fmt;
use std::fmt::{Display, Formatter};

macro_rules! parse_option {
    ($option:expr, $index:expr) => {
        if let Some(_) = $option {
        } else {
            return Err(ParseError::new($index));
        }
    };
}

#[derive(Debug)]
pub struct ParseError {
    index: usize,
}
impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at index {}", self.index)
    }
}
impl Error for ParseError {}
impl ParseError {
    pub fn new(index: usize) -> Self {
        Self { index }
    }
}

pub struct Parser {
    index: usize,
    text: Vec<char>,
    current_char: char,
    section_nesting: u8,
    section_return: Option<u8>,
}

impl Parser {
    pub fn new(text: String) -> Self {
        let mut text: Vec<char> = text.chars().collect();
        text.append(&mut vec!['\n', ' ', '\n']); // push space and newline of eof. it fixes stuff and I don't know why.
        let current_char = text.get(0).unwrap().clone();

        Self {
            index: 0,
            text,
            current_char,
            section_nesting: 0,
            section_return: None,
        }
    }

    /// Increments the current index and returns the
    /// char at the indexes position
    pub fn next_char(&mut self) -> Option<char> {
        self.index += 1;

        self.current_char = self.text.get(self.index)?.clone();

        Some(self.current_char)
    }

    /// Returns to an index position
    pub fn revert_to(&mut self, index: usize) -> Result<(), ParseError> {
        if let Some(char) = self.text.get(index) {
            self.index = index;
            self.current_char = char.clone();
            Ok(())
        } else {
            Err(ParseError::new(index))
        }
    }

    /// Skips characters until it encounters a character
    /// that isn't an inline whitespace character
    pub fn seek_inline_whitespace(&mut self) {
        if self.current_char.is_whitespace() && self.current_char != '\n' {
            while let Some(next_char) = self.next_char() {
                if !next_char.is_whitespace() || self.current_char == '\n' {
                    break;
                }
            }
        }
    }

    /// Skips characters until it encounters a character
    /// that isn't a whitespace character
    pub fn seek_whitespace(&mut self) {
        if self.current_char.is_whitespace() {
            while let Some(next_char) = self.next_char() {
                if !next_char.is_whitespace() {
                    break;
                }
            }
        }
    }

    pub fn parse(&mut self) -> Document {
        let mut document = Document::new();
        while self.index < self.text.len() {
            if let Ok(token) = self.parse_block() {
                document.add_element(token);
            }
        }

        document
    }

    /// Parses a block Token
    pub fn parse_block(&mut self) -> Result<Block, ParseError> {
        if let Some(section) = self.section_return {
            if section <= self.section_nesting {
                return Err(ParseError::new(self.index));
            } else {
                self.section_return = None;
            }
        }
        let token = if let Ok(section) = self.parse_section() {
            Block::Section(section)
        } else if let Some(_) = self.section_return {
            return Err(ParseError::new(self.index));
        } else if let Ok(list) = self.parse_list() {
            Block::List(list)
        } else if let Ok(table) = self.parse_table() {
            Block::Table(table)
        } else if let Ok(paragraph) = self.parse_paragraph() {
            Block::Paragraph(paragraph)
        } else {
            return Err(ParseError::new(self.index));
        };

        Ok(token)
    }

    /// Parses a section that consists of a header and one or more blocks
    fn parse_section(&mut self) -> Result<Section, ParseError> {
        let start_index = self.index;
        self.seek_whitespace();
        if self.current_char == '#' {
            let mut size = 1;
            while let Some(next_char) = self.next_char() {
                if next_char == '#' {
                    size += 1;
                } else {
                    break;
                }
            }
            if size <= self.section_nesting || !self.current_char.is_whitespace() {
                let index = self.index;
                if size <= self.section_nesting {
                    self.section_return = Some(size);
                }
                self.revert_to(start_index)?;
                return Err(ParseError::new(index));
            }
            self.seek_inline_whitespace();
            let mut header = self.parse_header()?;
            header.size = size;
            self.section_nesting = size;
            let mut section = Section::new(header);

            while let Ok(block) = self.parse_block() {
                section.add_element(block);
            }

            self.section_nesting -= 1;
            Ok(section)
        } else {
            let error_index = self.index;
            self.revert_to(start_index)?;
            Err(ParseError::new(error_index))
        }
    }

    fn parse_header(&mut self) -> Result<Header, ParseError> {
        Ok(Header {
            size: 0,
            line: self.parse_inline()?,
        })
    }

    /// Parses a paragraph
    fn parse_paragraph(&mut self) -> Result<Paragraph, ParseError> {
        let mut paragraph = Paragraph::new();
        while let Ok(token) = self.parse_inline() {
            paragraph.add_element(token);
            let start_index = self.index;
            self.seek_inline_whitespace();
            if ['-', '#', '`', '|'].contains(&self.current_char) {
                self.revert_to(start_index)?;
                break;
            }
            self.revert_to(start_index)?;
        }

        if paragraph.elements.len() > 0 {
            Ok(paragraph)
        } else {
            Err(ParseError::new(self.index))
        }
    }

    /// parses a list which consists of one or more list items
    /// The parsing is done iterative to resolve nested items
    fn parse_list(&mut self) -> Result<List, ParseError> {
        let mut list = List::new();
        let start_index = self.index;
        self.seek_whitespace();

        let mut list_hierarchy: Vec<ListItem> = Vec::new();
        while let Ok(mut item) = self.parse_list_item() {
            while let Some(parent_item) = list_hierarchy.pop() {
                if parent_item.level < item.level {
                    // the parent item is the actual parent of the next item
                    list_hierarchy.push(parent_item);
                    break;
                } else if parent_item.level == item.level {
                    // the parent item is a sibling and has to be appended to a parent
                    if list_hierarchy.is_empty() {
                        list.add_item(parent_item);
                    } else {
                        let mut parent_parent = list_hierarchy.pop().unwrap();
                        parent_parent.add_child(parent_item);
                        list_hierarchy.push(parent_parent);
                    }
                    break;
                } else {
                    // the parent item is a child of a sibling of the current item
                    if list_hierarchy.is_empty() {
                        item.add_child(parent_item);
                    } else {
                        let mut parent_parent = list_hierarchy.pop().unwrap();
                        parent_parent.add_child(parent_item);
                        list_hierarchy.push(parent_parent);
                    }
                }
            }
            list_hierarchy.push(item);
        }

        // the remaining items in the hierarchy need to be combined
        while let Some(item) = list_hierarchy.pop() {
            if !list_hierarchy.is_empty() {
                let mut parent_item = list_hierarchy.pop().unwrap();
                parent_item.add_child(item);
                list_hierarchy.push(parent_item);
            } else {
                list_hierarchy.push(item);
                break;
            }
        }
        list.items.append(&mut list_hierarchy);

        if list.items.len() > 0 {
            Ok(list)
        } else {
            self.revert_to(start_index)?;
            Err(ParseError::new(self.index))
        }
    }

    /// parses a single list item defined with -
    fn parse_list_item(&mut self) -> Result<ListItem, ParseError> {
        let start_index = self.index;
        self.seek_inline_whitespace();
        let level = self.index - start_index;

        if !['-'].contains(&self.current_char) {
            let err = ParseError::new(self.index);
            self.revert_to(start_index)?;
            return Err(err);
        }
        if self.next_char() == None {
            let err = ParseError::new(self.index);
            self.revert_to(start_index)?;
            return Err(err);
        }
        self.seek_inline_whitespace();
        let item = ListItem::new(self.parse_inline()?, level as u16);

        Ok(item)
    }

    fn parse_table(&mut self) -> Result<Table, ParseError> {
        let header = self.parse_row()?;
        let start_index = self.index;
        self.seek_whitespace();
        if self.current_char == '-' {
            if self.next_char() != Some('|') {
                let err_index = self.index;
                self.revert_to(start_index)?;
                return Err(ParseError::new(err_index));
            }
        }
        while let Some(char) = self.next_char() {
            if char == '\n' {
                break;
            }
        }
        self.seek_whitespace();
        let mut table = Table::new(header);

        while let Ok(row) = self.parse_row() {
            table.add_row(row);
            self.seek_whitespace();
        }

        Ok(table)
    }

    /// parses a table row/head
    pub fn parse_row(&mut self) -> Result<Row, ParseError> {
        let start_index = self.index;
        self.seek_inline_whitespace();

        if self.current_char == '|' {
            if self.next_char() == None {
                let err_index = self.index;
                self.revert_to(start_index)?;
                return Err(ParseError::new(err_index));
            }
        } else {
            self.revert_to(start_index)?;
            return Err(ParseError::new(self.index));
        }
        let mut row = Row::new();
        while let Ok(element) = self.parse_inline() {
            row.add_cell(Cell { text: element });
            if self.current_char == '|' {
                if self.next_char() == None {
                    break;
                }
            }
            if self.current_char == '\n' {
                break;
            }
        }

        if row.cells.len() > 0 {
            Ok(row)
        } else {
            let current_index = self.index;
            self.revert_to(start_index)?;
            Err(ParseError::new(current_index))
        }
    }

    fn parse_inline(&mut self) -> Result<Inline, ParseError> {
        if self.index > self.text.len() {
            Err(ParseError::new(self.index))
        } else {
            Ok(Inline::Text(self.parse_text()?))
        }
    }

    /// Parses a line of text
    fn parse_text(&mut self) -> Result<Text, ParseError> {
        let mut text = Text::new();
        while let Ok(subtext) = self.parse_subtext() {
            text.add_subtext(subtext);
            let current_index = self.index;
            if self.next_char() == None {
                break;
            }
            self.revert_to(current_index)?;
        }

        if self.current_char == '\n' {
            parse_option!(self.next_char(), self.index);
        }

        Ok(text)
    }

    fn parse_subtext(&mut self) -> Result<SubText, ParseError> {
        match self.current_char {
            '*' => {
                parse_option!(self.next_char(), self.index);

                if self.current_char == '*' {
                    parse_option!(self.next_char(), self.index);
                    let subtext = self.parse_subtext()?;
                    if self.current_char == '*' {
                        parse_option!(self.next_char(), self.index);
                        if self.current_char == '*' {
                            parse_option!(self.next_char(), self.index);
                        }
                    }
                    Ok(SubText::Bold(BoldText {
                        value: Box::new(subtext),
                    }))
                } else {
                    let subtext = self.parse_subtext()?;
                    parse_option!(self.next_char(), self.index);
                    Ok(SubText::Italic(ItalicText {
                        value: Box::new(subtext),
                    }))
                }
            }
            '_' => {
                parse_option!(self.next_char(), self.index);
                let subtext = self.parse_subtext()?;
                parse_option!(self.next_char(), self.index);
                Ok(SubText::Underlined(UnderlinedText {
                    value: Box::new(subtext),
                }))
            }
            '~' => {
                parse_option!(self.next_char(), self.index);
                let subtext = self.parse_subtext()?;
                if self.current_char == '~' {
                    parse_option!(self.next_char(), self.index);
                }
                Ok(SubText::Striked(StrikedText {
                    value: Box::new(subtext),
                }))
            }
            '\n' | '|' => Err(ParseError::new(self.index)),
            _ => Ok(SubText::Plain(self.parse_plain_text()?)),
        }
    }

    fn parse_plain_text(&mut self) -> Result<PlainText, ParseError> {
        let mut current_char = self.current_char;
        let mut characters = String::new();
        loop {
            match current_char {
                '\n' | '*' | '_' | '~' | '|' => break,
                _ => characters.push(current_char),
            }
            if let Some(character) = self.next_char() {
                current_char = character;
            } else {
                break;
            }
        }

        if characters.len() > 0 {
            Ok(PlainText { value: characters })
        } else {
            Err(ParseError::new(self.index))
        }
    }
}
