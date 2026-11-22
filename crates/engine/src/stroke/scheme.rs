//! 笔画组合：键入的笔画与通配符，以及 `msime-stroke.db` 对它们给出的单字或词组候选。每个字是一段笔画前缀，撇号把多个字的段分开。

use super::{glyph, is_key, is_stroke, key_of_glyph, MAX_STROKES, WILDCARD};
use crate::error::Result;
use crate::language_dictionary::{LanguageDictionary, LanguageEntry};
use crate::types::{QueryRequest, SchemeKey, SchemeType};

/// 笔画码与键入完全相同（含通配匹配到同样长度）的字最多读这么多。
pub const EXACT_LIMIT: usize = 200;
/// 以键入笔画开头、笔画更多的字最多读这么多。
pub const COMPLETION_LIMIT: usize = 100;
/// 笔画词组候选最多读这么多。词组查询本身已经按词库权重排序。
pub const PHRASE_LIMIT: usize = 200;

/// 一个候选字或词和它在 `msime-stroke.db` 里的笔画码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrokeCandidate {
    pub text: String,
    pub weight: i64,
    /// 该字完整的笔画码（`entries.key`）。
    pub key: String,
}

#[derive(Debug, Clone, Default)]
pub struct StrokeScheme {
    /// `hspnz`、`x` 与词组分隔用的 `'`，至多 `MAX_STROKES` 个输入字节。键入时 `x` 不会出现在整个组合开头；宿主编辑可能把它留在开头。
    input: String,
}

/// 会话持有的字典查询行，重复文字也保留，以便下次查询复用它们的字符串。
#[derive(Default)]
pub(crate) struct StrokeQueryBuffer {
    entries: Vec<LanguageEntry>,
    exact_pattern: Vec<(String, LanguageEntry)>,
    completions: Vec<(String, LanguageEntry)>,
    phrases: Vec<(String, LanguageEntry)>,
}

impl StrokeQueryBuffer {
    fn query(&mut self, dictionary: &LanguageDictionary, input: &str) -> Result<bool> {
        let wildcard = char::from(WILDCARD);
        let has_wildcard = input.contains(wildcard);
        if has_wildcard {
            dictionary.lookup_pattern_into(
                input,
                wildcard,
                false,
                EXACT_LIMIT,
                &mut self.exact_pattern,
            )?;
            dictionary.lookup_pattern_into(
                input,
                wildcard,
                true,
                COMPLETION_LIMIT,
                &mut self.completions,
            )?;
        } else {
            dictionary.lookup_into(input, EXACT_LIMIT, &mut self.entries)?;
            dictionary.lookup_completions_into(input, COMPLETION_LIMIT, &mut self.completions)?;
        }
        Ok(has_wildcard)
    }

    fn query_phrase(&mut self, dictionary: &LanguageDictionary, segments: &[&str]) -> Result<()> {
        dictionary.lookup_stroke_phrase_into(
            segments,
            char::from(WILDCARD),
            PHRASE_LIMIT,
            &mut self.phrases,
        )
    }
}

impl StrokeScheme {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.input.clear();
    }

    /// 笔画键追加一笔；通配符只在已有组合时追加；撇号分隔词组中的字；Backspace 删最后一个笔画或分隔符。达到 `MAX_STROKES` 后不再追加。其他键（别的字母、大写、符号）都不改变组合，是否吞掉由会话决定。
    pub fn handle_key(&mut self, key: SchemeKey) -> bool {
        match key {
            SchemeKey::Letter(letter) if is_stroke(letter) => self.push(letter),
            SchemeKey::Letter(WILDCARD) if !self.input.is_empty() => self.push(WILDCARD),
            SchemeKey::Apostrophe => self.push_delimiter(),
            SchemeKey::Backspace => self.input.pop().is_some(),
            SchemeKey::Letter(_)
            | SchemeKey::Semicolon
            | SchemeKey::Minus
            | SchemeKey::Symbol(_)
            | SchemeKey::Requery => false,
        }
    }

    fn push(&mut self, key: u8) -> bool {
        if self.input.len() < MAX_STROKES {
            self.input.push(char::from(key));
            true
        } else {
            false
        }
    }

    pub fn can_push_delimiter(&self) -> bool {
        self.input.len() < MAX_STROKES
            && self.input.as_bytes().last().is_some_and(|key| is_key(*key))
    }

    fn push_delimiter(&mut self) -> bool {
        if self.can_push_delimiter() {
            self.input.push('\'');
            true
        } else {
            false
        }
    }

    /// 用宿主编辑后的文本替换组合：笔画键、通配符和合法的撇号分隔符原样保留，笔画字形换回对应的键，其他字符丢弃，超出 `MAX_STROKES` 的部分截掉。
    pub fn set_raw_input(&mut self, raw: &str) {
        self.input.clear();
        self.input.reserve(raw.len().min(MAX_STROKES));
        for character in raw.chars() {
            if character == '\'' {
                self.push_delimiter();
                continue;
            }
            let key = match u8::try_from(character) {
                Ok(byte) if is_key(byte) => Some(byte),
                _ => key_of_glyph(character),
            };
            if let Some(key) = key {
                self.push(key);
            }
        }
    }

    /// 键入的笔画字母和词组分隔符（`hspnzx'`），Enter 上屏的就是它。
    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// 组合显示的笔画字形，例如 `一丨＊`；词组分隔符保留为 `'`。
    pub fn glyphs(&self) -> String {
        let mut output = String::new();
        self.glyphs_into(&mut output);
        output
    }

    pub fn glyphs_into(&self, output: &mut String) {
        output.clear();
        for character in self.input.chars() {
            if character == '\'' {
                output.push('\'');
            } else if let Ok(key) = u8::try_from(character) {
                if let Some(glyph) = glyph(key) {
                    output.push(glyph);
                }
            }
        }
    }

    /// 会话刷新用的请求。`raw_input` 是键入的字母，也是宿主编辑的文本；`normalized_segmentation` 是快照 `reading` 带给宿主绘制的笔画字形。
    pub fn build_request(&self) -> QueryRequest {
        let mut request = QueryRequest::default();
        self.build_request_into(&mut request);
        request
    }

    pub fn build_request_into(&self, request: &mut QueryRequest) {
        request.scheme = SchemeType::Stroke;
        request.raw_input.clone_from(&self.input);
        request.raw_input_with_cases.clone_from(&self.input);
        request.normalized_input.clone_from(&self.input);
        request.raw_segmentation.clone_from(&self.input);
        self.glyphs_into(&mut request.normalized_segmentation);
        request.segmentation.clone_from(&self.input);
        request.valid = self.input.bytes().any(is_key);
    }

    /// 会话显示并据以判断是否在组合的文本：笔画字形，没有组合时为空。
    pub fn preedit(&self) -> String {
        self.glyphs()
    }

    /// 组合的候选，每个字或词只列一次：
    /// 1. 有字频、笔画码与键入完全相同的字（通配符匹配任意一笔），重的在前；
    /// 2. 有字频、以键入笔画开头、笔画更多的字，重的在前；
    /// 3. 没有字频的字（权重为 0 的生僻字），同样精确匹配在前、补全在后。
    ///
    /// 笔数恰好打满时，生僻字的精确匹配不再挡在常用字的补全前面。
    /// 有撇号时改查词组：撇号两边各是一字的笔画前缀，支持每个字使用不同长度的前缀。
    pub fn candidates(&self, dictionary: &LanguageDictionary) -> Result<Vec<StrokeCandidate>> {
        let input = self.input.as_str();
        if input.is_empty() {
            return Ok(Vec::new());
        }
        if input.contains('\'') {
            let mut buffer = StrokeQueryBuffer::default();
            let mut candidates = Vec::new();
            self.phrase_candidates_into(dictionary, &mut buffer, &mut candidates)?;
            return Ok(candidates);
        }
        let mut buffer = StrokeQueryBuffer::default();
        buffer.query(dictionary, input)?;
        let unique_len = {
            let mut seen: [Option<&str>; EXACT_LIMIT + COMPLETION_LIMIT] =
                [None; EXACT_LIMIT + COMPLETION_LIMIT];
            let mut unique_len = 0;
            for text in buffer
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .chain(
                    buffer
                        .exact_pattern
                        .iter()
                        .map(|(_, entry)| entry.text.as_str()),
                )
                .chain(
                    buffer
                        .completions
                        .iter()
                        .map(|(_, entry)| entry.text.as_str()),
                )
            {
                if !seen[..unique_len]
                    .iter()
                    .flatten()
                    .any(|seen| *seen == text)
                {
                    seen[unique_len] = Some(text);
                    unique_len += 1;
                }
            }
            unique_len
        };
        let mut candidates = Vec::with_capacity(unique_len);
        for entry in buffer.entries {
            push_owned_candidate(&mut candidates, input, entry);
        }
        for (key, entry) in buffer.exact_pattern.into_iter().chain(buffer.completions) {
            push_owned_candidate(&mut candidates, key, entry);
        }
        candidates.sort_by_key(|candidate| candidate.weight <= 0);
        Ok(candidates)
    }

    /// 复用查询行与候选字符串；查询失败时清空候选，不暴露上次结果。
    pub(crate) fn candidates_into(
        &self,
        dictionary: &LanguageDictionary,
        buffer: &mut StrokeQueryBuffer,
        candidates: &mut Vec<StrokeCandidate>,
    ) -> Result<()> {
        let input = self.input.as_str();
        if input.is_empty() {
            candidates.clear();
            return Ok(());
        }
        if input.contains('\'') {
            return self.phrase_candidates_into(dictionary, buffer, candidates);
        }
        let has_wildcard = match buffer.query(dictionary, input) {
            Ok(has_wildcard) => has_wildcard,
            Err(error) => {
                candidates.clear();
                return Err(error);
            }
        };
        let exact_len = if has_wildcard {
            buffer.exact_pattern.len()
        } else {
            buffer.entries.len()
        };
        let row_count = exact_len + buffer.completions.len();
        candidates.reserve(row_count.saturating_sub(candidates.len()));
        // 先按查询顺序去重，保留首次行；不能让高权重补全替代零或负权重精确行。
        let mut unique: [Option<(&str, &LanguageEntry)>; EXACT_LIMIT + COMPLETION_LIMIT] =
            [None; EXACT_LIMIT + COMPLETION_LIMIT];
        debug_assert!(row_count <= unique.len());
        let mut unique_length = 0;
        for index in 0..row_count {
            let (key, entry) = if index >= exact_len {
                let (key, entry) = &buffer.completions[index - exact_len];
                (key.as_str(), entry)
            } else if has_wildcard {
                let (key, entry) = &buffer.exact_pattern[index];
                (key.as_str(), entry)
            } else {
                (input, &buffer.entries[index])
            };
            if unique[..unique_length]
                .iter()
                .flatten()
                .any(|(_, previous)| previous.text == entry.text)
            {
                continue;
            }
            unique[unique_length] = Some((key, entry));
            unique_length += 1;
        }
        // 直接按最终稳定顺序覆写，避免排序移动字符串容量后下次热查询又扩容。
        let mut length = 0;
        for non_positive in [false, true] {
            for &(key, entry) in unique[..unique_length].iter().flatten() {
                if (entry.weight <= 0) == non_positive {
                    push_candidate(candidates, &mut length, key, entry);
                }
            }
        }
        candidates.truncate(length);
        Ok(())
    }

    fn phrase_candidates_into(
        &self,
        dictionary: &LanguageDictionary,
        buffer: &mut StrokeQueryBuffer,
        candidates: &mut Vec<StrokeCandidate>,
    ) -> Result<()> {
        candidates.clear();
        let input = self.input.as_str();
        let trailing = input.ends_with('\'');
        let segments: Vec<&str> = input
            .split('\'')
            .filter(|segment| !segment.is_empty())
            .collect();
        if segments.is_empty() {
            return Ok(());
        }
        if trailing {
            let mut base = Vec::new();
            if segments.len() == 1 {
                StrokeScheme {
                    input: segments[0].to_owned(),
                }
                .candidates_into(dictionary, buffer, &mut base)?;
            } else {
                non_trailing_phrase_candidates_into(&segments, dictionary, buffer, &mut base)?;
            }
            let mut base_iter = base.into_iter();
            if let Some(first) = base_iter.next() {
                candidates.push(first);
            }
            if let Err(error) = dictionary.lookup_stroke_phrase_continuations_into(
                &segments,
                char::from(WILDCARD),
                20,
                &mut buffer.phrases,
            ) {
                candidates.clear();
                return Err(error);
            }
            for (key, entry) in buffer.phrases.drain(..) {
                push_owned_candidate(candidates, key, entry);
            }
            for candidate in base_iter {
                if !contains_text(candidates, &candidate.text) {
                    candidates.push(candidate);
                }
            }
            return Ok(());
        }
        non_trailing_phrase_candidates_into(&segments, dictionary, buffer, candidates)
    }
}

fn non_trailing_phrase_candidates_into(
    segments: &[&str],
    dictionary: &LanguageDictionary,
    buffer: &mut StrokeQueryBuffer,
    candidates: &mut Vec<StrokeCandidate>,
) -> Result<()> {
    candidates.clear();
    if let Err(error) = buffer.query_phrase(dictionary, segments) {
        candidates.clear();
        return Err(error);
    }
    for (key, entry) in buffer.phrases.drain(..) {
        push_owned_candidate(candidates, key, entry);
    }
    if segments.len() <= 16 && candidates.len() < 3 {
        let tier1_empty = candidates.is_empty();
        assemble_stroke_sentence(dictionary, segments, tier1_empty, candidates)?;
    }
    Ok(())
}

/// 词组表未命中或候选不足时，用动态规划把多段笔画拼成整句（先长词再高频单字），并在无整词命中时展开末段备选。
fn assemble_stroke_sentence(
    dictionary: &LanguageDictionary,
    segments: &[&str],
    emit_tail_variants: bool,
    candidates: &mut Vec<StrokeCandidate>,
) -> Result<()> {
    const PHRASE_COHESION_BONUS: i128 = 200_000;

    #[derive(Clone)]
    struct DpNode {
        score: i128,
        text: String,
        key: String,
        last_start: usize,
        last_weight: i64,
    }

    let n = segments.len();
    if n < 2 {
        return Ok(());
    }
    let mut dp: Vec<Option<DpNode>> = vec![None; n + 1];
    dp[0] = Some(DpNode {
        score: 0,
        text: String::new(),
        key: String::new(),
        last_start: 0,
        last_weight: 0,
    });
    let mut scratch_buffer = StrokeQueryBuffer::default();
    let mut single_buf = Vec::new();
    let wildcard = char::from(WILDCARD);

    for i in 0..n {
        if dp[i].is_none() {
            continue;
        }
        for span_len in 1..=4.min(n - i) {
            if span_len == n {
                continue;
            }
            let j = i + span_len;
            let (span_key, span_text, span_weight) = if span_len == 1 {
                StrokeScheme {
                    input: segments[i].to_owned(),
                }
                .candidates_into(
                    dictionary,
                    &mut scratch_buffer,
                    &mut single_buf,
                )?;
                let Some(best_char) = single_buf.first() else {
                    continue;
                };
                (
                    best_char.key.clone(),
                    best_char.text.clone(),
                    best_char.weight,
                )
            } else {
                dictionary.lookup_stroke_phrase_into(
                    &segments[i..j],
                    wildcard,
                    1,
                    &mut scratch_buffer.phrases,
                )?;
                let Some((key, entry)) = scratch_buffer.phrases.first() else {
                    continue;
                };
                (key.clone(), entry.text.clone(), entry.weight)
            };
            let span_score = i128::try_from(span_len - 1).unwrap_or(0) * PHRASE_COHESION_BONUS
                + i128::from(span_weight.max(1));
            let prev = dp[i].as_ref().expect("checked non-none above");
            let total_score = prev.score + span_score;
            if dp[j]
                .as_ref()
                .is_none_or(|existing| total_score > existing.score)
            {
                let mut next_text = String::with_capacity(prev.text.len() + span_text.len());
                next_text.push_str(&prev.text);
                next_text.push_str(&span_text);
                let mut next_key = String::with_capacity(prev.key.len() + 1 + span_key.len());
                if !prev.key.is_empty() {
                    next_key.push_str(&prev.key);
                    next_key.push(' ');
                }
                next_key.push_str(&span_key);
                dp[j] = Some(DpNode {
                    score: total_score,
                    text: next_text,
                    key: next_key,
                    last_start: i,
                    last_weight: span_weight,
                });
            }
        }
    }

    let Some(best) = dp[n].as_ref() else {
        return Ok(());
    };
    if !contains_text(candidates, &best.text) {
        candidates.push(StrokeCandidate {
            text: best.text.clone(),
            weight: best.last_weight.max(1),
            key: best.key.clone(),
        });
    }
    if !emit_tail_variants || best.last_start >= n {
        return Ok(());
    }
    let prefix = dp[best.last_start].as_ref();
    let prefix_text = prefix.map_or("", |node| node.text.as_str());
    let prefix_key = prefix.map_or("", |node| node.key.as_str());
    let tail = &segments[best.last_start..n];
    let mut added = 0usize;
    if tail.len() == 1 {
        StrokeScheme {
            input: tail[0].to_owned(),
        }
        .candidates_into(dictionary, &mut scratch_buffer, &mut single_buf)?;
        for cand in single_buf.drain(..) {
            let full_text = format!("{prefix_text}{}", cand.text);
            if contains_text(candidates, &full_text) {
                continue;
            }
            let full_key = if prefix_key.is_empty() {
                cand.key
            } else {
                format!("{prefix_key} {}", cand.key)
            };
            candidates.push(StrokeCandidate {
                text: full_text,
                weight: cand.weight.max(1),
                key: full_key,
            });
            added += 1;
            if added >= 7 {
                break;
            }
        }
    } else {
        dictionary.lookup_stroke_phrase_into(tail, wildcard, 8, &mut scratch_buffer.phrases)?;
        for (key, entry) in scratch_buffer.phrases.drain(..) {
            let full_text = format!("{prefix_text}{}", entry.text);
            if contains_text(candidates, &full_text) {
                continue;
            }
            let full_key = if prefix_key.is_empty() {
                key
            } else {
                format!("{prefix_key} {key}")
            };
            candidates.push(StrokeCandidate {
                text: full_text,
                weight: entry.weight.max(1),
                key: full_key,
            });
            added += 1;
            if added >= 7 {
                break;
            }
        }
    }
    Ok(())
}

fn contains_text(candidates: &[StrokeCandidate], text: &str) -> bool {
    candidates.iter().any(|candidate| candidate.text == text)
}

fn push_owned_candidate(
    candidates: &mut Vec<StrokeCandidate>,
    key: impl Into<String>,
    entry: LanguageEntry,
) {
    if contains_text(candidates, &entry.text) {
        return;
    }
    candidates.push(StrokeCandidate {
        key: key.into(),
        text: entry.text,
        weight: entry.weight,
    });
}

fn push_candidate(
    candidates: &mut Vec<StrokeCandidate>,
    length: &mut usize,
    key: &str,
    entry: &LanguageEntry,
) {
    if let Some(candidate) = candidates.get_mut(*length) {
        candidate.key.clear();
        candidate.key.push_str(key);
        candidate.text.clear();
        candidate.text.push_str(&entry.text);
        candidate.weight = entry.weight;
    } else {
        candidates.push(StrokeCandidate {
            key: key.to_owned(),
            text: entry.text.clone(),
            weight: entry.weight,
        });
    }
    *length += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_dictionary::open_read_only;
    use crate::stroke::fixture;

    struct Fixture {
        _dir: tempfile::TempDir,
        dictionary: LanguageDictionary,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("msime-stroke.db");
        fixture::build(&path);
        Fixture {
            dictionary: open_read_only(&path).unwrap(),
            _dir: dir,
        }
    }

    fn typed(keys: &str) -> StrokeScheme {
        let mut scheme = StrokeScheme::new();
        for key in keys.bytes() {
            scheme.handle_key(if key == b'\'' {
                SchemeKey::Apostrophe
            } else {
                SchemeKey::Letter(key)
            });
        }
        scheme
    }

    fn texts(scheme: &StrokeScheme, dictionary: &LanguageDictionary) -> Vec<String> {
        scheme
            .candidates(dictionary)
            .unwrap()
            .into_iter()
            .map(|candidate| candidate.text)
            .collect()
    }

    #[test]
    fn strokes_append_and_show_as_glyphs() {
        let scheme = typed("hspnz");
        assert_eq!(scheme.input(), "hspnz");
        assert_eq!(scheme.preedit(), "一丨丿丶乛");
        let request = scheme.build_request();
        assert_eq!(request.scheme, SchemeType::Stroke);
        assert_eq!(request.raw_input, "hspnz");
        assert_eq!(request.raw_input_with_cases, "hspnz");
        assert_eq!(request.normalized_segmentation, "一丨丿丶乛");
        assert!(request.valid);
        assert!(!StrokeScheme::new().build_request().valid);
        assert_eq!(StrokeScheme::new().preedit(), "");
    }

    #[test]
    fn other_keys_change_nothing() {
        let mut scheme = typed("h");
        for key in [
            SchemeKey::Letter(b'a'),
            SchemeKey::Letter(b'H'),
            SchemeKey::Letter(b'q'),
            SchemeKey::Semicolon,
            SchemeKey::Minus,
            SchemeKey::Symbol(b'*'),
            SchemeKey::Requery,
        ] {
            scheme.handle_key(key);
        }
        assert_eq!(scheme.input(), "h");
        assert!(scheme.handle_key(SchemeKey::Apostrophe));
        assert_eq!(scheme.input(), "h'");
        assert!(!scheme.handle_key(SchemeKey::Apostrophe));
    }

    #[test]
    fn the_wildcard_needs_a_stroke_before_it() {
        let mut scheme = typed("x");
        assert!(scheme.is_empty());
        scheme.handle_key(SchemeKey::Letter(b'h'));
        scheme.handle_key(SchemeKey::Letter(b'x'));
        scheme.handle_key(SchemeKey::Letter(b'x'));
        assert_eq!(scheme.input(), "hxx");
        assert_eq!(scheme.preedit(), "一＊＊");
    }

    #[test]
    fn backspace_removes_the_last_stroke_and_reset_clears() {
        let mut scheme = typed("hsx");
        scheme.handle_key(SchemeKey::Backspace);
        assert_eq!(scheme.input(), "hs");
        scheme.reset();
        assert!(scheme.is_empty());
        scheme.handle_key(SchemeKey::Backspace);
        assert!(scheme.is_empty());
    }

    #[test]
    fn the_composition_stops_at_the_stroke_limit() {
        let mut scheme = typed(&"h".repeat(MAX_STROKES));
        scheme.handle_key(SchemeKey::Letter(b's'));
        scheme.handle_key(SchemeKey::Letter(b'x'));
        assert_eq!(scheme.input().len(), MAX_STROKES);
        assert!(scheme.input().bytes().all(|key| key == b'h'));
        scheme.set_raw_input(&"s".repeat(MAX_STROKES + 5));
        assert_eq!(scheme.input().len(), MAX_STROKES);
    }

    #[test]
    fn host_edits_keep_keys_and_read_glyphs_back() {
        let mut scheme = StrokeScheme::new();
        scheme.set_raw_input("h丨a丿x1乛 ＊");
        assert_eq!(scheme.input(), "hspxzx");
        assert_eq!(scheme.preedit(), "一丨丿＊乛＊");
        scheme.set_raw_input("h丨'丿");
        assert_eq!(scheme.input(), "hs'p");
        assert_eq!(scheme.preedit(), "一丨'丿");
        // 宿主编辑留下的开头通配符保留，查询照样能用。
        scheme.set_raw_input("xs");
        assert_eq!(scheme.input(), "xs");
        scheme.set_raw_input("");
        assert!(scheme.is_empty());
    }

    #[test]
    fn exact_codes_come_before_longer_ones_and_texts_are_listed_once() {
        let fixture = fixture();
        let dictionary = &fixture.dictionary;
        // 十 的笔画码正好是 hs，排在更重的长码字前面；土 有两个笔画码，只列一次。
        assert_eq!(texts(&typed("hs"), dictionary), ["十", "土"]);
        assert_eq!(
            texts(&typed("h"), dictionary),
            ["一", "大", "二", "十", "三", "王", "土", "干"]
        );
        let candidates = typed("hs").candidates(dictionary).unwrap();
        assert_eq!(
            candidates[0],
            StrokeCandidate {
                text: "十".to_owned(),
                weight: 4500,
                key: "hs".to_owned(),
            }
        );
        assert_eq!(candidates[1].key, "hsh");
        assert!(texts(&typed("zzzz"), dictionary).is_empty());
        assert!(texts(&StrokeScheme::new(), dictionary).is_empty());
    }

    #[test]
    fn apostrophe_queries_phrase_prefixes_with_mixed_lengths() {
        let fixture = fixture();
        let dictionary = &fixture.dictionary;
        let scheme = typed("ph'ps");
        assert_eq!(scheme.preedit(), "丿一'丿丨");
        assert_eq!(texts(&scheme, dictionary), ["我们"]);
        let candidate = scheme.candidates(dictionary).unwrap().pop().unwrap();
        assert_eq!(candidate.key, "phshzpn psnsz");
        assert_eq!(
            typed("px'px").candidates(dictionary).unwrap()[0].text,
            "我们"
        );
        assert_eq!(scheme.build_request().normalized_segmentation, "丿一'丿丨");
    }

    #[test]
    fn owned_candidates_do_not_reserve_duplicate_query_rows() {
        let fixture = fixture();
        let candidates = typed("h").candidates(&fixture.dictionary).unwrap();

        assert_eq!(candidates.len(), 8);
        assert_eq!(
            candidates.capacity(),
            candidates.len(),
            "重复笔画码不应继续占用候选容量"
        );
    }

    #[test]
    fn empty_owned_query_does_not_allocate_page_buffers() {
        let fixture = fixture();
        for (input, expected_allocations) in [("zzzz", 3), ("zzzx", 6)] {
            let scheme = typed(input);
            scheme.candidates(&fixture.dictionary).unwrap();
            let (candidates, allocations) = crate::ime::personal_rerank::allocations::count(|| {
                scheme.candidates(&fixture.dictionary).unwrap()
            });
            assert!(candidates.is_empty());
            assert_eq!(
                allocations, expected_allocations,
                "空笔画查询 {input} 申请了 {allocations} 个缓冲"
            );
        }
    }

    #[test]
    fn reused_candidates_keep_first_duplicate_before_stable_frequency_partition() {
        let fixture = fixture();
        let connection =
            rusqlite::Connection::open(fixture._dir.path().join("msime-stroke.db")).unwrap();
        connection
            .execute("DELETE FROM entries WHERE key NOT IN ('hs', 'hsh')", [])
            .unwrap();
        for (key, text, weight) in [
            ("hs", "重複零", 0),
            ("hsh", "重複零", 10000),
            ("hs", "重複負", -1),
            ("hsh", "重複負", 9000),
            ("hsh", "補全零", 0),
            ("hsh", "補全負", -2),
        ] {
            connection
                .execute(
                    "INSERT INTO entries VALUES (?1, ?2, ?3)",
                    (key, text, weight),
                )
                .unwrap();
        }
        drop(connection);
        let mut buffer = StrokeQueryBuffer::default();
        let mut candidates = Vec::new();
        for input in ["hs", "hx", "hs"] {
            let scheme = typed(input);
            scheme
                .candidates_into(&fixture.dictionary, &mut buffer, &mut candidates)
                .unwrap();
            assert_eq!(
                candidates
                    .iter()
                    .map(|row| (row.text.as_str(), row.key.as_str(), row.weight))
                    .collect::<Vec<_>>(),
                [
                    ("十", "hs", 4500),
                    ("土", "hsh", 2000),
                    ("重複零", "hs", 0),
                    ("重複負", "hs", -1),
                    ("補全零", "hsh", 0),
                    ("補全負", "hsh", -2)
                ]
            );
            assert_eq!(candidates, scheme.candidates(&fixture.dictionary).unwrap());
        }
        typed("zzzz")
            .candidates_into(&fixture.dictionary, &mut buffer, &mut candidates)
            .unwrap();
        assert!(candidates.is_empty());
        typed("h")
            .candidates_into(&fixture.dictionary, &mut buffer, &mut candidates)
            .unwrap();
        assert!(!candidates.is_empty());
        StrokeScheme::new()
            .candidates_into(&fixture.dictionary, &mut buffer, &mut candidates)
            .unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn characters_without_a_frequency_follow_every_weighted_one() {
        let fixture = fixture();
        let dictionary = &fixture.dictionary;
        // 乚 的笔画码正好是 sz，但没有字频，排在笔画更多、有字频的 口 后面。
        assert_eq!(texts(&typed("sz"), dictionary), ["口", "乚"]);
        // 通配符同样如此。
        assert_eq!(texts(&typed("sx"), dictionary), ["口", "乚"]);
    }

    #[test]
    fn the_wildcard_matches_any_one_stroke() {
        let fixture = fixture();
        let dictionary = &fixture.dictionary;
        // hx：两笔的 二 十 在前，再是 h 开头、笔画更多的字。
        assert_eq!(
            texts(&typed("hx"), dictionary),
            ["二", "十", "大", "三", "王", "土", "干"]
        );
        assert_eq!(texts(&typed("hxh"), dictionary), ["三", "土"]);
        let mut scheme = StrokeScheme::new();
        scheme.set_raw_input("xn");
        assert_eq!(texts(&scheme, dictionary), ["人"]);
    }

    #[test]
    fn trailing_delimiter_and_dynamic_sentence_assembly_work_with_phrases() {
        let fixture = fixture();
        let dictionary = &fixture.dictionary;
        let connection =
            rusqlite::Connection::open(fixture._dir.path().join("msime-stroke.db")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO entries VALUES\
                 ('pn pn', '人人', 7500),\
                 ('pnhhzn hpsn pnhhzn', '会不会', 9900),\
                 ('phshzpn', '我', 9500);",
            )
            .unwrap();
        drop(connection);

        // 末尾刚输入分隔符 `ph'`：首位保留当前字 `我`，紧接着词组续写 `我们`。
        let trailing_single = typed("ph'");
        assert_eq!(texts(&trailing_single, dictionary), ["我", "我们"]);

        // 精确两字词组 `pn'pn` 只返回 `人人`，不能被三字词 `会不会` 污染。
        assert_eq!(texts(&typed("pn'pn"), dictionary), ["人人"]);

        // 词库没有的组合也能按整句动态拼接，并展开末段备选。
        assert_eq!(texts(&typed("pn'hs"), dictionary), ["人十", "人土"]);
        assert_eq!(texts(&typed("ph'ps'pn"), dictionary), ["我们人"]);
    }
}
