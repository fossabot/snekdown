pub(crate) mod block;
pub(crate) mod charstate;
pub(crate) mod inline;
pub(crate) mod line;

use self::block::ParseBlock;
use crate::elements::{Document, ImportAnchor};
use crate::references::configuration::Configuration;
use crate::utils::parsing::{ParseError, ParseResult};
use colored::*;
use crossbeam_utils::sync::WaitGroup;
use std::fs::File;
use std::io;
use std::io::{BufRead, BufReader, Cursor};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

pub struct Parser {
    pub(crate) index: usize,
    pub(crate) text: Vec<char>,
    pub(crate) current_char: char,
    section_nesting: u8,
    sections: Vec<u8>,
    section_return: Option<u8>,
    path: Option<PathBuf>,
    paths: Arc<Mutex<Vec<PathBuf>>>,
    wg: WaitGroup,
    is_child: bool,
    pub(crate) block_break_at: Vec<char>,
    pub(crate) inline_break_at: Vec<char>,
    pub(crate) document: Document,
    pub(crate) previous_char: char,
    pub(crate) reader: Box<dyn BufRead>,
    pub(crate) parse_variables: bool,
}

impl Parser {
    /// Creates a new parser from a path
    pub fn new_from_file(path: PathBuf) -> Result<Self, io::Error> {
        let f = File::open(&path)?;
        Ok(Self::create(
            Some(PathBuf::from(path)),
            Arc::new(Mutex::new(Vec::new())),
            false,
            Box::new(BufReader::new(f)),
        ))
    }

    /// Creates a new parser with text being the markdown text
    pub fn new(text: String, path: Option<PathBuf>) -> Self {
        let text_bytes = text.as_bytes();
        let path = if let Some(inner_path) = path {
            Some(PathBuf::from(inner_path))
        } else {
            None
        };
        Parser::create(
            path,
            Arc::new(Mutex::new(Vec::new())),
            false,
            Box::new(Cursor::new(text_bytes.to_vec())),
        )
    }

    /// Creates a child parser from string text
    pub fn child(text: String, path: PathBuf, paths: Arc<Mutex<Vec<PathBuf>>>) -> Self {
        let text_bytes = text.as_bytes();
        Self::create(
            Some(PathBuf::from(path)),
            paths,
            true,
            Box::new(Cursor::new(text_bytes.to_vec())),
        )
    }

    /// Creates a child parser from a file
    pub fn child_from_file(
        path: PathBuf,
        paths: Arc<Mutex<Vec<PathBuf>>>,
    ) -> Result<Self, io::Error> {
        let f = File::open(&path)?;
        Ok(Self::create(
            Some(PathBuf::from(path)),
            paths,
            true,
            Box::new(BufReader::new(f)),
        ))
    }

    fn create(
        path: Option<PathBuf>,
        paths: Arc<Mutex<Vec<PathBuf>>>,
        is_child: bool,
        mut reader: Box<dyn BufRead>,
    ) -> Self {
        if let Some(path) = path.clone() {
            paths.lock().unwrap().push(path.clone())
        }
        let mut text = Vec::new();
        let mut current_char = ' ';
        for _ in 0..8 {
            let mut buf = String::new();
            if let Ok(_) = reader.read_line(&mut buf) {
                text.append(&mut buf.chars().collect::<Vec<char>>());
            } else {
                break;
            }
        }
        if let Some(ch) = text.get(0) {
            current_char = *ch
        }
        let document = Document::new(!is_child);
        Self {
            index: 0,
            text,
            current_char,
            sections: Vec::new(),
            section_nesting: 0,
            section_return: None,
            path,
            paths,
            wg: WaitGroup::new(),
            is_child,
            previous_char: ' ',
            inline_break_at: Vec::new(),
            block_break_at: Vec::new(),
            document,
            reader,
            parse_variables: false,
        }
    }

    pub fn set_config(&mut self, config: Configuration) {
        self.document.config = config;
    }

    /// Returns the text of the parser as a string
    fn get_text(&self) -> String {
        self.text
            .iter()
            .fold("".to_string(), |a, b| format!("{}{}", a, b))
    }

    /// Returns the import paths of the parser
    pub fn get_paths(&self) -> Vec<PathBuf> {
        self.paths.lock().unwrap().clone()
    }

    /// transform an import path to be relative to the current parsers file
    fn transform_path(&mut self, path: String) -> PathBuf {
        let mut path = PathBuf::from(path);

        if !path.is_absolute() {
            if let Some(selfpath) = &self.path {
                if let Some(dir) = selfpath.parent() {
                    path = PathBuf::new().join(dir).join(path);
                }
            }
        }

        path
    }

    /// starts up a new thread to parse the imported document
    fn import_document(&mut self, path: String) -> ParseResult<Arc<RwLock<ImportAnchor>>> {
        let path = self.transform_path(path);
        if !path.exists() || !path.is_file() {
            println!(
                "{}",
                format!(
                    "Import of \"{}\" failed: The file doesn't exist.",
                    path.to_str().unwrap()
                )
                .red()
            );
            return Err(ParseError::new_with_message(
                self.index,
                "file does not exist",
            ));
        }
        {
            let mut paths = self.paths.lock().unwrap();
            if paths.iter().find(|item| **item == path) != None {
                println!(
                    "{}",
                    format!(
                        "Import of \"{}\" failed: Cyclic import.",
                        path.to_str().unwrap()
                    )
                    .yellow()
                );
                return Err(ParseError::new_with_message(self.index, "cyclic import"));
            }
            paths.push(path.clone());
        }
        let anchor = Arc::new(RwLock::new(ImportAnchor::new()));
        let anchor_clone = Arc::clone(&anchor);
        let wg = self.wg.clone();
        let paths = Arc::clone(&self.paths);
        let config = self.document.config.clone();

        let _ = thread::spawn(move || {
            let mut parser = Parser::child_from_file(path, paths).unwrap();
            parser.set_config(config);
            let document = parser.parse();
            anchor_clone.write().unwrap().set_document(document);

            drop(wg);
        });

        Ok(anchor)
    }

    /// parses the given text into a document
    pub fn parse(&mut self) -> Document {
        self.document.path = if let Some(path) = &self.path {
            Some(path.canonicalize().unwrap().to_str().unwrap().to_string())
        } else {
            None
        };

        while self.index < self.text.len() {
            match self.parse_block() {
                Ok(block) => self.document.add_element(block),
                Err(err) => {
                    if err.eof {
                        break;
                    }
                    if let Some(path) = &self.path {
                        if let Some(position) = err.get_position(&self.get_text()) {
                            println!(
                                "{}",
                                format!(
                                    "Error in File {}:{}:{} - {}",
                                    path.to_str().unwrap(),
                                    position.0,
                                    position.1,
                                    err
                                )
                                .red()
                            );
                        } else {
                            println!(
                                "{}",
                                format!("Error in File {}: {}", path.to_str().unwrap(), err).red()
                            );
                        }
                    } else {
                        println!("{}", err);
                    }
                    break;
                }
            }
        }

        let wg = self.wg.clone();
        self.wg = WaitGroup::new();
        wg.wait();
        self.document.post_process();
        let document = self.document.clone();
        self.document = Document::new(!self.is_child);

        document
    }
}
