use std::sync::Arc;

use crate::{
    collections::{HashMap, HashSet},
    inline_parser::annotation::{self, Annotation, ByteRange, CommentText, ParsingError},
    location::Buffer,
};

pub struct SourceLines {
    line_starts: Vec<usize>,
}

impl SourceLines {
    pub fn new(source: &str) -> SourceLines {
        let mut line_starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        SourceLines { line_starts }
    }

    pub fn line(&self, position: usize) -> usize {
        match self.line_starts.binary_search(&position) {
            Ok(index) => index + 1,
            Err(index) => index,
        }
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line - 1]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Comment {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug)]
pub struct LocatedAnnotation {
    pub annotation: Annotation,
    pub location: ByteRange,
}

#[derive(Clone, Debug)]
pub struct AnnotationSyntaxError {
    pub location: ByteRange,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum Paragraph {
    Text,
    Annotation(LocatedAnnotation),
    SyntaxError(AnnotationSyntaxError),
}

#[derive(Clone, Debug)]
pub enum TrailingAnnotation {
    Annotation(LocatedAnnotation),
    SyntaxError(AnnotationSyntaxError),
}

pub struct CommentBlock {
    buffer: Arc<Buffer>,
    comments: Vec<(Comment, usize)>,
    content: String,
    line_ranges: Vec<(usize, usize)>,
    leading: bool,
    start_line: usize,
    end_line: usize,
    first_comment_starts_with_colon_or_bracket: bool,
}

impl CommentBlock {
    fn new(buffer: &Arc<Buffer>, lines: &SourceLines, comments: Vec<Comment>) -> CommentBlock {
        let source = buffer.content.as_str();
        let mut content = String::new();
        let mut line_ranges = Vec::new();
        let mut with_prefix = Vec::new();
        for comment in comments {
            let text = &source[comment.start..comment.end];
            let prefix = if text.starts_with("# ") { 2 } else { 1 };
            let line = &source[comment.start + prefix..comment.end];
            let line_start = content.len();
            content.push_str(line);
            line_ranges.push((line_start, content.len()));
            content.push('\n');
            with_prefix.push((comment, prefix));
        }
        content.pop();
        let first = with_prefix[0].0;
        let last = with_prefix[with_prefix.len() - 1].0;
        let first_line_start = lines.line_start(lines.line(first.start));
        let leading = source[first_line_start..first.start]
            .bytes()
            .all(|byte| byte.is_ascii_whitespace());
        let first_text = &source[first.start..first.end];
        CommentBlock {
            buffer: buffer.clone(),
            content,
            line_ranges,
            leading,
            start_line: lines.line(first.start),
            end_line: lines.line(last.end),
            first_comment_starts_with_colon_or_bracket: first_text.starts_with("#:")
                || first_text.starts_with("#["),
            comments: with_prefix,
        }
    }

    pub fn build(
        buffer: &Arc<Buffer>,
        lines: &SourceLines,
        comments: &[Comment],
    ) -> Vec<CommentBlock> {
        let source = buffer.content.as_str();
        let mut blocks = Vec::new();
        let mut index = 0;
        while index < comments.len() {
            let mut block_comments: Vec<Comment> = vec![comments[index]];
            index += 1;
            while index < comments.len() {
                let comment = comments[index];
                let last_comment = block_comments[block_comments.len() - 1];
                let comment_line = lines.line(comment.start);
                let comment_column = comment.start - lines.line_start(comment_line);
                let last_line = lines.line(last_comment.start);
                let last_column = last_comment.start - lines.line_start(last_line);
                let only_comment_on_line = source[lines.line_start(comment_line)..comment.start]
                    .bytes()
                    .all(|byte| byte.is_ascii_whitespace());
                if lines.line(last_comment.end) + 1 == comment_line
                    && last_column == comment_column
                    && only_comment_on_line
                {
                    block_comments.push(comment);
                    index += 1;
                    continue;
                }
                break;
            }
            blocks.push(CommentBlock::new(buffer, lines, block_comments));
        }
        blocks
    }

    pub fn is_leading(&self) -> bool {
        self.leading
    }

    pub fn start_line(&self) -> usize {
        self.start_line
    }

    pub fn end_line(&self) -> usize {
        self.end_line
    }

    fn line_count(&self) -> usize {
        self.line_ranges.len()
    }

    fn text(&self, index: usize) -> &str {
        let (start, end) = self.line_ranges[index];
        &self.content[start..end]
    }

    fn original_position(&self, position: usize) -> usize {
        let index = self
            .line_ranges
            .iter()
            .position(|(_, end)| position <= *end)
            .unwrap_or(self.line_ranges.len() - 1);
        let (comment, prefix) = self.comments[index];
        comment.start + prefix + (position - self.line_ranges[index].0)
    }

    fn original_range(&self, range: ByteRange) -> ByteRange {
        ByteRange {
            start: self.original_position(range.start),
            end: self.original_position(range.end),
        }
    }

    fn line_location(&self, start_line: usize, end_line: usize) -> ByteRange {
        self.original_range(ByteRange {
            start: self.line_ranges[start_line].0,
            end: self.line_ranges[end_line].1,
        })
    }

    fn is_leading_annotation(&self, index: usize, source: &str) -> bool {
        if index >= self.line_count() {
            return false;
        }
        if let Some(rest) = self.text(index).strip_prefix("@rbs")
            && !rest
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return true;
        }
        let (comment, _) = self.comments[index];
        source[comment.start..comment.end].starts_with("#:")
    }

    fn parse_annotation<T>(
        &self,
        parse: impl FnOnce(&CommentText) -> Result<T, ParsingError>,
    ) -> Result<T, ParsingError> {
        let position = |position| self.original_position(position);
        parse(&CommentText {
            content: &self.content,
            buffer: &self.buffer,
            position: &position,
        })
    }

    fn located(&self, annotation: Annotation) -> LocatedAnnotation {
        LocatedAnnotation {
            location: self.original_range(annotation.location()),
            annotation,
        }
    }

    fn syntax_error(&self, location: ByteRange, error: ParsingError) -> AnnotationSyntaxError {
        AnnotationSyntaxError {
            location,
            message: format!("Syntax error: {}", error.message),
        }
    }

    fn parse_annotation_lines(&self, start_line: usize, end_line: usize) -> Option<Paragraph> {
        let start = self.line_ranges[start_line].0;
        let end = self.line_ranges[end_line].1;
        match self.parse_annotation(|text| annotation::parse_leading(text, start, end)) {
            Ok(Some(annotation)) => Some(Paragraph::Annotation(self.located(annotation))),
            Ok(None) => None,
            Err(error) => Some(Paragraph::SyntaxError(
                self.syntax_error(self.line_location(start_line, end_line), error),
            )),
        }
    }

    pub fn each_paragraph(&self, source: &str) -> Vec<Paragraph> {
        let mut paragraphs = Vec::new();
        if self.is_leading_annotation(0, source) {
            self.yield_annotation(0, 0, 0, source, &mut paragraphs);
        } else {
            self.yield_paragraph(0, source, &mut paragraphs);
        }
        paragraphs
    }

    fn yield_paragraph(
        &self,
        mut current_line: usize,
        source: &str,
        paragraphs: &mut Vec<Paragraph>,
    ) {
        loop {
            let next_line = current_line + 1;
            if next_line >= self.line_count() {
                paragraphs.push(Paragraph::Text);
                return;
            }
            if self.is_leading_annotation(next_line, source) {
                paragraphs.push(Paragraph::Text);
                return self.yield_annotation(next_line, next_line, next_line, source, paragraphs);
            }
            current_line = next_line;
        }
    }

    fn yield_annotation(
        &self,
        start_line: usize,
        mut end_line: usize,
        mut current_line: usize,
        source: &str,
        paragraphs: &mut Vec<Paragraph>,
    ) {
        loop {
            let next_line = current_line + 1;
            if next_line >= self.line_count() {
                paragraphs.extend(self.parse_annotation_lines(start_line, end_line));
                if end_line > current_line {
                    self.yield_paragraph(end_line + 1, source, paragraphs);
                }
                return;
            }
            let line_text = self.text(next_line);
            match line_text
                .bytes()
                .position(|byte| !byte.is_ascii_whitespace())
            {
                Some(0) => {
                    paragraphs.extend(self.parse_annotation_lines(start_line, end_line));
                    if self.is_leading_annotation(end_line + 1, source) {
                        self.yield_annotation(
                            end_line + 1,
                            end_line + 1,
                            end_line + 1,
                            source,
                            paragraphs,
                        );
                    } else {
                        self.yield_paragraph(end_line + 1, source, paragraphs);
                    }
                    return;
                }
                Some(_) => {
                    current_line = next_line;
                    end_line = next_line;
                }
                None => current_line = next_line,
            }
        }
    }

    pub fn trailing_annotation(&self) -> Option<TrailingAnnotation> {
        if self.leading || !self.first_comment_starts_with_colon_or_bracket {
            return None;
        }
        let end = self.content.len();
        match self.parse_annotation(|text| annotation::parse_trailing(text, 0, end)) {
            Ok(Some(annotation)) => Some(TrailingAnnotation::Annotation(self.located(annotation))),
            Ok(None) => None,
            Err(error) => {
                let location = self.line_location(0, self.line_count() - 1);
                Some(TrailingAnnotation::SyntaxError(
                    self.syntax_error(location, error),
                ))
            }
        }
    }
}

pub struct CommentAssociation {
    pub blocks: Vec<CommentBlock>,
    associated: HashSet<usize>,
    start_line_map: HashMap<usize, usize>,
    end_line_map: HashMap<usize, usize>,
}

impl CommentAssociation {
    pub fn new(mut blocks: Vec<CommentBlock>) -> CommentAssociation {
        blocks.sort_by_key(|block| block.start_line);
        let mut start_line_map = HashMap::default();
        let mut end_line_map = HashMap::default();
        for (index, block) in blocks.iter().enumerate() {
            if block.leading {
                end_line_map.insert(block.end_line, index);
            } else {
                start_line_map.insert(block.start_line, index);
            }
        }
        CommentAssociation {
            blocks,
            associated: HashSet::default(),
            start_line_map,
            end_line_map,
        }
    }

    pub fn block(&self, index: usize) -> &CommentBlock {
        &self.blocks[index]
    }

    pub fn leading_block(&self, start_line: usize) -> Option<usize> {
        self.end_line_map
            .get(&(start_line.checked_sub(1)?))
            .copied()
    }

    pub fn associate(&mut self, index: usize) {
        self.associated.insert(index);
    }

    pub fn is_associated(&self, index: usize) -> bool {
        self.associated.contains(&index)
    }

    pub fn leading_block_associating(&mut self, start_line: usize) -> Option<usize> {
        let index = self.leading_block(start_line)?;
        if self.is_associated(index) {
            return None;
        }
        self.associate(index);
        Some(index)
    }

    pub fn trailing_block_associating(&mut self, end_line: usize) -> Option<usize> {
        let index = *self.start_line_map.get(&end_line)?;
        if self.is_associated(index) {
            return None;
        }
        self.associate(index);
        Some(index)
    }

    pub fn enclosed_blocks_associating(
        &mut self,
        start_line: usize,
        end_line: usize,
    ) -> Vec<usize> {
        let mut indexes = Vec::new();
        if start_line + 1 < end_line {
            for line in (start_line + 1)..end_line {
                if let Some(&index) = self.end_line_map.get(&line)
                    && !self.associated.contains(&index)
                {
                    self.associated.insert(index);
                    indexes.push(index);
                }
            }
        }
        indexes
    }

    pub fn unassociated_blocks(&self) -> Vec<usize> {
        (0..self.blocks.len())
            .filter(|index| !self.associated.contains(index))
            .collect()
    }
}
