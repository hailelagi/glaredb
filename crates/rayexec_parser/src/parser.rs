use crate::{
    ast::{Expr, Ident, Literal, ObjectReference},
    keywords::{self, Keyword, RESERVED_FOR_COLUMN_ALIAS, RESERVED_FOR_TABLE_ALIAS},
    statement::Statement,
    tokens::{Token, TokenWithLocation, Tokenizer, Word},
};
use rayexec_error::{RayexecError, Result};
use std::fmt;

/// Parse a sql query into statements.
pub fn parse(sql: &str) -> Result<Vec<Statement<'_>>> {
    let toks = Tokenizer::new(sql).tokenize()?;
    Parser::with_tokens(toks).parse_statements()
}

#[derive(Debug)]
pub struct Parser<'a> {
    toks: Vec<TokenWithLocation<'a>>,
    /// Index of token we should process next.
    idx: usize,
}

impl<'a> Parser<'a> {
    pub fn with_tokens(toks: Vec<TokenWithLocation<'a>>) -> Self {
        Parser { toks, idx: 0 }
    }

    /// Parse any number of statements, including zero statements.
    ///
    /// Statements are expected to be delineated with a semicolon.
    pub fn parse_statements(&mut self) -> Result<Vec<Statement<'a>>> {
        let mut stmts = Vec::new();
        let mut expect_delimiter = false;

        loop {
            while self.consume_token(Token::SemiColon) {
                expect_delimiter = false;
            }

            if self.peek().is_none() {
                // We're done.
                break;
            }

            if expect_delimiter {
                return Err(RayexecError::new("Expected semicolon between statements"));
            }

            let stmt = self.parse_statement()?;
            stmts.push(stmt);

            expect_delimiter = true;
        }

        Ok(stmts)
    }

    /// Parse a single statement.
    pub fn parse_statement(&mut self) -> Result<Statement<'a>> {
        let tok = match self.next() {
            Some(tok) => tok,
            None => return Err(RayexecError::new("Empty SQL statement")),
        };

        match &tok.token {
            Token::Word(word) => {
                let keyword = match word.keyword {
                    Some(k) => k,
                    None => {
                        return Err(RayexecError::new(format!(
                            "Expected a keyword, got {}",
                            word.value,
                        )))
                    }
                };

                match keyword {
                    Keyword::CREATE => self.parse_create(),
                    Keyword::SET => self.parse_set(),
                    other => {
                        return Err(RayexecError::new(format!("Unexpected keyword: {other:?}",)))
                    }
                }
            }
            other => {
                return Err(RayexecError::new(format!(
                    "Expected a SQL statement, got {other:?}"
                )))
            }
        }
    }

    pub fn parse_create(&mut self) -> Result<Statement<'a>> {
        let or_replace = self.parse_keyword_sequence(&[Keyword::OR, Keyword::REPLACE]);
        let temp = self.parse_one_of_keywords(&[Keyword::TEMP, Keyword::TEMPORARY]);

        if self.parse_keyword(Keyword::TABLE) {
            // Table
            unimplemented!()
        } else if self.parse_keyword(Keyword::SCHEMA) {
            // Schema
            if or_replace {
                return Err(RayexecError::new(
                    "OR REPLACE not supported when creating a schema",
                ));
            }
            if temp {
                return Err(RayexecError::new(
                    "TEMPORARY not supported when creating a schema",
                ));
            }

            let if_not_exists =
                self.parse_keyword_sequence(&[Keyword::IF, Keyword::NOT, Keyword::EXISTS]);
            let reference = self.parse_object_reference()?;

            Ok(Statement::CreateSchema {
                reference,
                if_not_exists,
            })
        } else {
            unimplemented!()
        }
    }

    pub fn parse_set(&mut self) -> Result<Statement<'a>> {
        let name = self.parse_object_reference()?;
        if self.parse_keyword(Keyword::TO) || self.consume_token(Token::Eq) {
            let expr = self.parse_expr()?;
            return Ok(Statement::SetVariable {
                reference: name,
                value: expr,
            });
        }

        Err(RayexecError::new(format!(
            "Expected 'SET {name} TO <value>' or SET {name} = <value>'"
        )))
    }

    /// Parse an object reference.
    fn parse_object_reference(&mut self) -> Result<ObjectReference<'a>> {
        let mut idents = Vec::new();
        loop {
            let tok = match self.next() {
                Some(tok) => tok,
                None => break,
            };
            let ident = match &tok.token {
                Token::Word(w) => Ident { value: w.value },
                other => {
                    return Err(RayexecError::new(format!(
                        "Unexpected token: {other:?}. Expected an object reference.",
                    )))
                }
            };
            idents.push(ident);

            // Check if the next token is a period for possible compound
            // identifiers. If not, we're done.
            if !self.consume_token(Token::Period) {
                break;
            }
        }

        Ok(ObjectReference(idents))
    }

    /// Parse a sql expression.
    fn parse_expr(&mut self) -> Result<Expr<'a>> {
        let expr = self.parse_prefix_expr()?;

        // TODO: Infix

        Ok(expr)
    }

    fn parse_prefix_expr(&mut self) -> Result<Expr<'a>> {
        // TODO: Typed string

        let tok = match self.next() {
            Some(tok) => tok,
            None => {
                return Err(RayexecError::new(
                    "Unexpected end of statement. Expected expression.",
                ))
            }
        };

        let expr = match &tok.token {
            Token::Word(w) => match w.keyword {
                Some(kw) => match kw {
                    Keyword::TRUE => Expr::Literal(Literal::Boolean(true)),
                    Keyword::FALSE => Expr::Literal(Literal::Boolean(false)),
                    Keyword::NULL => Expr::Literal(Literal::Null),
                    _ => unimplemented!(),
                },
                None => {
                    unimplemented!()
                }
            },
            Token::SingleQuotedString(s) => Expr::Literal(Literal::SingleQuotedString(s)),
            Token::Number(s) => Expr::Literal(Literal::Number(s)),
            other => {
                return Err(RayexecError::new(format!(
                    "Unexpected token '{other:?}'. Expected expression."
                )))
            }
        };

        Ok(expr)
    }

    /// Parse a comma-separated list of one or more items.
    fn parse_comma_separated<T>(
        &mut self,
        mut f: impl FnMut(&mut Parser<'a>) -> Result<T>,
    ) -> Result<Vec<T>> {
        let mut values = Vec::new();
        loop {
            values.push(f(self)?);
            if !self.consume_token(Token::Comma) {
                break;
            }

            let tok = match self.peek() {
                Some(tok) => &tok.token,
                None => break,
            };

            match tok {
                Token::RightParen | Token::SemiColon | Token::RightBrace | Token::RightBracket => {
                    break
                }
                Token::Word(w) => {
                    if let Some(kw) = &w.keyword {
                        if RESERVED_FOR_COLUMN_ALIAS
                            .iter()
                            .any(|reserved| reserved == kw)
                        {
                            break;
                        }
                    }
                }
                _ => (),
            }
        }

        Ok(values)
    }

    /// Parse a single keyword.
    fn parse_keyword(&mut self, keyword: Keyword) -> bool {
        let idx = self.idx;
        if let Some(tok) = self.next() {
            if tok.is_keyword(keyword) {
                return true;
            }
        }

        // Keyword doesn't match. Reset index and return.
        self.idx = idx;
        false
    }

    /// Parse an exact sequence of keywords.
    ///
    /// If the sequence doesn't match, idx is not changed, and false is
    /// returned.
    fn parse_keyword_sequence(&mut self, keywords: &[Keyword]) -> bool {
        let idx = self.idx;
        for keyword in keywords {
            if let Some(tok) = self.next() {
                if tok.is_keyword(*keyword) {
                    continue;
                }
            }

            // Keyword doesn't match. Reset index and return.
            self.idx = idx;
            return false;
        }
        true
    }

    /// Parse any of the provided keywords.
    fn parse_one_of_keywords(&mut self, keywords: &[Keyword]) -> bool {
        let idx = self.idx;
        let tok = match self.next() {
            Some(tok) => tok,
            None => return false,
        };

        if keywords.iter().any(|k| tok.is_keyword(*k)) {
            return true;
        }

        // No matches, reset index.
        self.idx = idx;
        false
    }

    /// Consume the next token if it matches expected.
    ///
    /// Returns false with the state unchanged if the next token does not match
    /// expected.
    fn consume_token(&mut self, expected: Token) -> bool {
        let tok = match self.peek() {
            Some(tok) => &tok.token,
            None => return false,
        };
        if tok == &expected {
            let _ = self.next();
            return true;
        }
        false
    }

    /// Get the next token.
    ///
    /// Ignores whitespace.
    fn next(&mut self) -> Option<&TokenWithLocation<'a>> {
        loop {
            if self.idx >= self.toks.len() {
                return None;
            }

            let tok = &self.toks[self.idx];
            self.idx += 1;

            if matches!(&tok.token, Token::Whitespace) {
                continue;
            }

            return Some(tok);
        }
    }

    /// Get the next token without altering the current index.
    ///
    /// Ignores whitespace.
    fn peek(&mut self) -> Option<&TokenWithLocation<'a>> {
        let mut idx = self.idx;
        loop {
            if idx >= self.toks.len() {
                return None;
            }

            let tok = &self.toks[idx];
            idx += 1;

            if matches!(&tok.token, Token::Whitespace) {
                continue;
            }

            return Some(tok);
        }
    }
}
