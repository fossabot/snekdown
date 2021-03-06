use crate::elements::tokens::*;
use crate::elements::{Block, CodeBlock, Import, List, ListItem, Paragraph, Quote, Section, Table};
use crate::parser::charstate::CharStateMachine;
use crate::parser::inline::ParseInline;
use crate::parser::line::ParseLine;
use crate::utils::parsing::{ParseError, ParseResult};
use crate::Parser;

pub(crate) trait ParseBlock {
    fn parse_block(&mut self) -> ParseResult<Block>;
    fn parse_section(&mut self) -> ParseResult<Section>;
    fn parse_code_block(&mut self) -> ParseResult<CodeBlock>;
    fn parse_quote(&mut self) -> ParseResult<Quote>;
    fn parse_paragraph(&mut self) -> ParseResult<Paragraph>;
    fn parse_list(&mut self) -> ParseResult<List>;
    fn parse_table(&mut self) -> ParseResult<Table>;
    fn parse_import(&mut self) -> ParseResult<Import>;
}

impl ParseBlock for Parser {
    /// Parses a block Token
    fn parse_block(&mut self) -> ParseResult<Block> {
        if let Some(section) = self.section_return {
            if section <= self.section_nesting && (self.section_nesting > 0) {
                return Err(ParseError::new_with_message(
                    self.index,
                    "invalid section nesting",
                ));
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
        } else if let Ok(code_block) = self.parse_code_block() {
            Block::CodeBlock(code_block)
        } else if let Ok(quote) = self.parse_quote() {
            Block::Quote(quote)
        } else if let Ok(import) = self.parse_import() {
            Block::Import(import)
        } else if let Some(_) = self.section_return {
            return Err(ParseError::new(self.index));
        } else if let Ok(pholder) = self.parse_placeholder() {
            Block::Placeholder(pholder)
        } else if let Ok(paragraph) = self.parse_paragraph() {
            Block::Paragraph(paragraph)
        } else {
            return Err(ParseError::new(self.index));
        };

        Ok(token)
    }

    /// Parses a section that consists of a header and one or more blocks
    fn parse_section(&mut self) -> ParseResult<Section> {
        let start_index = self.index;
        self.seek_whitespace();
        if self.check_special(&HASH) {
            let mut size = 1;
            while let Some(_) = self.next_char() {
                if !self.check_special(&HASH) {
                    break;
                }
                size += 1;
            }
            let mut metadata = None;
            if let Ok(meta) = self.parse_inline_metadata() {
                metadata = Some(meta);
            }
            if size <= self.section_nesting || !self.current_char.is_whitespace() {
                if size <= self.section_nesting {
                    self.section_return = Some(size);
                }
                return Err(self.revert_with_error(start_index));
            }
            self.seek_inline_whitespace();
            let mut header = self.parse_header()?;
            header.size = size;
            self.section_nesting = size;
            self.sections.push(size);
            let mut section = Section::new(header);
            section.metadata = metadata;
            self.seek_whitespace();

            while let Ok(block) = self.parse_block() {
                section.add_element(block);
            }

            self.sections.pop();
            if let Some(sec) = self.sections.last() {
                self.section_nesting = *sec
            } else {
                self.section_nesting = 0;
            }
            Ok(section)
        } else {
            return Err(self.revert_with_error(start_index));
        }
    }

    /// parses a code block
    fn parse_code_block(&mut self) -> ParseResult<CodeBlock> {
        self.seek_whitespace();
        self.assert_special_sequence(&SQ_CODE_BLOCK, self.index)?;
        self.skip_char();
        let language = self.get_string_until(&[LB], &[])?;
        self.skip_char();
        let text = self.get_string_until_sequence(&[&SQ_CODE_BLOCK], &[])?;
        for _ in 0..2 {
            self.skip_char();
        }

        Ok(CodeBlock {
            language,
            code: text,
        })
    }

    /// parses a quote
    fn parse_quote(&mut self) -> ParseResult<Quote> {
        let start_index = self.index;
        self.seek_whitespace();
        let metadata = if let Ok(meta) = self.parse_inline_metadata() {
            Some(meta)
        } else {
            None
        };
        if self.check_special(&META_CLOSE) {
            if self.next_char() == None {
                return Err(self.revert_with_error(start_index));
            }
        }
        let mut quote = Quote::new(metadata);

        while self.check_special(&QUOTE_START)
            && self.next_char() != None
            && (self.check_seek_inline_whitespace() || self.check_special(&LB))
        {
            if let Ok(text) = self.parse_text_line() {
                if text.subtext.len() > 0 {
                    quote.add_text(text);
                }
            } else {
                break;
            }
        }
        if quote.text.len() == 0 {
            return Err(self.revert_with_error(start_index));
        }

        Ok(quote)
    }

    /// Parses a paragraph
    fn parse_paragraph(&mut self) -> ParseResult<Paragraph> {
        self.seek_whitespace();
        let mut paragraph = Paragraph::new();
        while let Ok(token) = self.parse_line() {
            paragraph.add_element(token);
            let start_index = self.index;
            if self.check_special_sequence_group(&BLOCK_SPECIAL_CHARS)
                || self.check_special_group(&self.block_break_at)
            {
                self.revert_to(start_index)?;
                break;
            }
            if !self.check_eof() {
                self.revert_to(start_index)?;
            }
        }

        if paragraph.elements.len() > 0 {
            Ok(paragraph)
        } else {
            Err(ParseError::new(self.index))
        }
    }

    /// parses a list which consists of one or more list items
    /// The parser is done iterative to resolve nested items
    fn parse_list(&mut self) -> ParseResult<List> {
        let mut list = List::new();
        let start_index = self.index;
        self.seek_whitespace();

        let ordered = if self.check_special_group(&LIST_SPECIAL_CHARS) {
            false
        } else {
            true
        };
        list.ordered = ordered;
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
            return Err(self.revert_with_error(start_index));
        }
    }

    /// parses a markdown table
    fn parse_table(&mut self) -> ParseResult<Table> {
        let header = self.parse_row()?;
        if self.check_linebreak() {
            self.skip_char();
        }
        let seek_index = self.index;
        let mut table = Table::new(header);
        while let Some(_) = self.next_char() {
            self.seek_inline_whitespace();
            if !self.check_special_group(&[MINUS, PIPE]) || self.check_linebreak() {
                break;
            }
        }

        if !self.check_linebreak() {
            self.revert_to(seek_index)?;
            return Ok(table);
        }

        self.seek_whitespace();
        while let Ok(row) = self.parse_row() {
            table.add_row(row);
        }

        Ok(table)
    }

    /// parses an import and starts a new task to parse the document of the import
    fn parse_import(&mut self) -> ParseResult<Import> {
        let start_index = self.index;
        self.seek_whitespace();
        self.assert_special_sequence_group(&[&[IMPORT_START, IMPORT_OPEN]], start_index)?;
        let mut path = String::new();
        while let Some(character) = self.next_char() {
            if self.check_linebreak() || self.check_special(&IMPORT_CLOSE) {
                break;
            }
            path.push(character);
        }
        if self.check_linebreak() || path.is_empty() {
            return Err(self.revert_with_error(start_index));
        }
        if self.check_special(&IMPORT_CLOSE) {
            self.skip_char();
        }
        // parser success

        if self.section_nesting > 0 {
            self.section_return = Some(0);
            let err = ParseError::new_with_message(self.index, "import section nesting error");
            self.revert_to(start_index)?;
            return Err(err);
        }

        self.seek_whitespace();

        if let Ok(anchor) = self.import_document(path.clone()) {
            Ok(Import { path, anchor })
        } else {
            Err(ParseError::new(self.index))
        }
    }
}
