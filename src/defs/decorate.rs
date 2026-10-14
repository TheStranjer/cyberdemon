//! The DECORATE parser (subset): `actor` headers, radius and spawn sprite.
//!
//! `scan` tokenises DECORATE-shaped text, skipping `//` and `/* */`
//! comments and quoted strings so braces inside them cannot shift the
//! parser's brace-depth tracking. `parse` walks the tokens for
//! `<keyword> <Name> [: <Parent>] [replaces <Other>] [<doomednum>] { ... }`
//! definitions and lifts out what the session definitions need: the
//! `Radius` and the sprite and first frame letter of the first state line
//! under `Spawn:` in the `States` block. Anything unparseable becomes a
//! warning. `ZScript` classes share the machinery through
//! [`crate::defs::zscript`].

/// The length of a sprite name.
const SPRITE_LEN: usize = 4;

/// The kind of a scanned token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A name-like run of letters, digits and underscores.
    Word,
    /// A run of digits with an optional decimal point.
    Number,
    /// Any other single character, braces and colons included.
    Punct,
}

/// One token of the [`scan`] output: its kind and text (one character for
/// punctuation).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Tok {
    /// Whether this is a word, a number or punctuation.
    kind: Kind,
    /// The token text.
    text: String,
}

/// What the header word under the cursor asks the header loop to do next.
#[derive(Clone, Copy, PartialEq, Eq)]
enum After {
    /// Nothing special.
    Nothing,
    /// The following word is the parent (`extends`).
    Parent,
    /// The following word is the replaced class (`replaces`).
    Skip,
}

/// One parsed class or actor definition, before inheritance resolution.
#[derive(Clone, Debug, Default)]
pub(crate) struct RawClass {
    /// The class name as written.
    pub(crate) name: String,
    /// The parent from `: Parent` or `extends Parent`, when given.
    pub(crate) parent: Option<String>,
    /// The doomednum from the header, DECORATE actors only.
    pub(crate) doomednum: Option<u32>,
    /// The four-letter sprite of the first `Spawn:` state, when found.
    pub(crate) sprite: Option<String>,
    /// The first frame letter of that state, when found.
    pub(crate) frame: Option<char>,
    /// The `Radius` of the body, when found.
    pub(crate) radius: Option<u32>,
}

/// The parser dialect and its warning sink.
struct Spec<'spec> {
    /// The label every warning starts with (`"decorate"`, `"zscript"`).
    prefix: &'spec str,
    /// Whether a header number names the actor's doomednum.
    doomednums: bool,
    /// The warnings gathered while parsing.
    messages: Vec<String>,
}

impl Spec<'_> {
    /// Records one warning carrying this dialect's prefix.
    fn warn(&mut self, message: String) {
        self.messages.push(message);
    }
}

/// A read position in a token slice.
struct Cursor<'tok> {
    /// The tokens being parsed.
    tokens: &'tok [Tok],
    /// The index of the token under the cursor.
    pos: usize,
}

impl<'tok> Cursor<'tok> {
    /// The token under the cursor.
    fn peek(&self) -> Option<&'tok Tok> {
        self.tokens.get(self.pos)
    }

    /// The word text under the cursor, `None` past the end or on other
    /// kinds.
    fn word(&self) -> Option<&'tok str> {
        let item = self.peek()?;
        if item.kind != Kind::Word {
            return None;
        }
        Some(item.text.as_str())
    }

    /// The number text under the cursor.
    fn number(&self) -> Option<&'tok str> {
        let item = self.peek()?;
        if item.kind != Kind::Number {
            return None;
        }
        Some(item.text.as_str())
    }

    /// Whether the cursor sits on the punctuation `ch`.
    fn is_punct(&self, ch: char) -> bool {
        self.peek()
            .is_some_and(|item| item.kind == Kind::Punct && item.text.starts_with(ch))
    }

    /// Moves to the next token.
    const fn step(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }
}

/// Tokenises DECORATE/`ZScript`-shaped text: comments and quoted strings are
/// dropped entirely, every other non-word character becomes one punct.
fn scan(text: &str) -> Vec<Tok> {
    let mut tokens: Vec<Tok> = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        scan_char(ch, &mut chars, &mut tokens);
    }
    tokens
}

/// The punctuation token for the single character `ch`.
fn punct(ch: char) -> Tok {
    Tok {
        kind: Kind::Punct,
        text: ch.to_string(),
    }
}

/// Handles one character outside a comment or string.
fn scan_char<T>(ch: char, chars: &mut core::iter::Peekable<T>, tokens: &mut Vec<Tok>)
where
    T: Iterator<Item = (usize, char)>,
{
    if ch == '/' {
        scan_slash(chars, tokens);
        return;
    }
    if ch == '"' {
        take_until_close(chars, '"');
        return;
    }
    if ch.is_ascii_alphabetic() || ch == '_' {
        collect_run(ch, false, chars, tokens);
        return;
    }
    if ch.is_ascii_digit() {
        collect_run(ch, true, chars, tokens);
        return;
    }
    if !ch.is_whitespace() {
        tokens.push(punct(ch));
    }
}

/// Whether `ch` continues a word token.
const fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Whether `ch` continues a number token.
const fn is_number_char(ch: char) -> bool {
    ch.is_ascii_digit() || ch == '.'
}

/// Collects a word or (when `numeric`) number token past its first char.
fn collect_run<T>(
    first: char,
    numeric: bool,
    chars: &mut core::iter::Peekable<T>,
    tokens: &mut Vec<Tok>,
) where
    T: Iterator<Item = (usize, char)>,
{
    let mut text = String::new();
    text.push(first);
    let keep = if numeric {
        is_number_char
    } else {
        is_word_char
    };
    text.push_str(&take_while(chars, keep));
    let kind = if numeric { Kind::Number } else { Kind::Word };
    tokens.push(Tok { kind, text });
}

/// Handles a `/` that was just consumed: `//` starts a line comment, `/*`
/// a block comment, anything else is punctuation.
fn scan_slash<T>(chars: &mut core::iter::Peekable<T>, tokens: &mut Vec<Tok>)
where
    T: Iterator<Item = (usize, char)>,
{
    match chars.peek().map(|&(_, ch)| ch) {
        Some('/') => take_until_close(chars, '\n'),
        Some('*') => {
            chars.next();
            skip_block_comment(chars);
        }
        _ => tokens.push(punct('/')),
    }
}

/// Drops characters up to and including the first `close`, or to the end.
fn take_until_close<T>(chars: &mut core::iter::Peekable<T>, close: char)
where
    T: Iterator<Item = (usize, char)>,
{
    for (_, ch) in chars.by_ref() {
        if ch == close {
            return;
        }
    }
}

/// Drops characters until the `*/` that closes a block comment.
fn skip_block_comment<T>(chars: &mut core::iter::Peekable<T>)
where
    T: Iterator<Item = (usize, char)>,
{
    while let Some((_, ch)) = chars.next() {
        if ch == '*' && chars.peek().is_some_and(|&(_, next)| next == '/') {
            chars.next();
            return;
        }
    }
}

/// Consumes and returns the characters `chars` holds while `keep` accepts
/// them, stopping (without consuming) at the first rejection.
fn take_while<T, F>(chars: &mut core::iter::Peekable<T>, keep: F) -> String
where
    T: Iterator<Item = (usize, char)>,
    F: Fn(char) -> bool,
{
    let mut out = String::new();
    while let Some(&(_, ch)) = chars.peek() {
        if !keep(ch) {
            break;
        }
        out.push(ch);
        chars.next();
    }
    out
}

/// Parses one source text's class definitions: `keyword` is the header
/// word (`"actor"` or `"class"`), `prefix` labels the warnings, and
/// `doomednums` says whether a header number is the actor's doomednum.
pub(crate) fn parse(
    text: &str,
    keyword: &str,
    prefix: &str,
    doomednums: bool,
) -> (Vec<RawClass>, Vec<String>) {
    let tokens = scan(text);
    let mut spec = Spec {
        prefix,
        doomednums,
        messages: Vec::new(),
    };
    let mut cursor = Cursor {
        tokens: &tokens,
        pos: 0,
    };
    let mut classes: Vec<RawClass> = Vec::new();
    let mut depth = 0_usize;
    while cursor.peek().is_some() {
        if cursor.is_punct('{') {
            depth = depth.saturating_add(1);
            cursor.step();
            continue;
        }
        if cursor.is_punct('}') {
            depth = depth.saturating_sub(1);
            cursor.step();
            continue;
        }
        let named = cursor
            .word()
            .is_some_and(|word| word.eq_ignore_ascii_case(keyword));
        if depth == 0 && named {
            cursor.step();
            classes.extend(parse_class(&mut cursor, &mut spec));
            depth = 0;
            continue;
        }
        cursor.step();
    }
    (classes, spec.messages)
}

/// Parses DECORATE `actor` definitions from one lump text.
pub(crate) fn parse_decorate(text: &str) -> (Vec<RawClass>, Vec<String>) {
    parse(text, "actor", "decorate", true)
}

/// One class definition at the cursor (the keyword was consumed).
fn parse_class(cursor: &mut Cursor<'_>, spec: &mut Spec<'_>) -> Option<RawClass> {
    let mut class = RawClass::default();
    if !parse_header(cursor, &mut class, spec) {
        let message = format!("{}: class definition without a usable name", spec.prefix);
        spec.warn(message);
        return None;
    }
    parse_body(cursor, &mut class, spec);
    Some(class)
}

/// Reads the header up to its opening brace; `false` when no named class
/// header closed properly.
fn parse_header(cursor: &mut Cursor<'_>, class: &mut RawClass, spec: &mut Spec<'_>) -> bool {
    let mut pending = After::Nothing;
    while cursor.peek().is_some() {
        if cursor.is_punct('{') {
            cursor.step();
            return !class.name.is_empty();
        }
        if cursor.is_punct('}') {
            return false;
        }
        pending = read_header_piece(cursor, class, spec, pending);
        cursor.step();
    }
    false
}

/// One header token under the cursor, given what the previous token asked
/// for: takes the name, parent, replaced class or doomednum and reports the
/// follow-up the next token owes.
fn read_header_piece(
    cursor: &Cursor<'_>,
    class: &mut RawClass,
    spec: &mut Spec<'_>,
    pending: After,
) -> After {
    if pending != After::Nothing {
        if pending == After::Parent {
            class.parent = cursor.word().map(str::to_owned);
        }
        return After::Nothing;
    }
    if cursor.is_punct(':') {
        return After::Parent;
    }
    let word = cursor.word();
    if let Some(text) = word {
        if text.eq_ignore_ascii_case("replaces") {
            return After::Skip;
        }
        if text.eq_ignore_ascii_case("extends") {
            return After::Parent;
        }
        if class.name.is_empty() {
            text.clone_into(&mut class.name);
        }
        return After::Nothing;
    }
    let number = cursor.number();
    if let Some(text) = number {
        if spec.doomednums && class.doomednum.is_none() {
            set_doomednum(text, class, spec);
        }
    }
    After::Nothing
}

/// Records a header doomednum, warning about one that does not parse.
fn set_doomednum(text: &str, class: &mut RawClass, spec: &mut Spec<'_>) {
    match number_value(text) {
        Some(value) => class.doomednum = Some(value),
        None => spec.warn(format!("{}: bad doomednum {text:?}", spec.prefix)),
    }
}

/// The parser state inside one class body.
#[derive(Clone, Copy, Default)]
struct BodyState {
    /// Braces open since the body started (`1` inside the body braces).
    depth: usize,
    /// The depth of the `States {` block, while inside it.
    states_at: Option<usize>,
    /// Whether `States` was just seen and its brace is still to come.
    pending_states: bool,
}

/// Reads the body up to and including the closing brace, taking the
/// `Radius` and the first `Spawn:` state line wherever they sit.
fn parse_body(cursor: &mut Cursor<'_>, class: &mut RawClass, spec: &mut Spec<'_>) {
    let mut state = BodyState {
        depth: 1,
        ..Default::default()
    };
    while cursor.peek().is_some() {
        if body_step(cursor, class, &mut state, spec) {
            return;
        }
    }
    spec.warn(format!(
        "{}: class {} ends inside its body",
        spec.prefix, class.name
    ));
}

/// One body token; `true` when the class body has closed.
fn body_step(
    cursor: &mut Cursor<'_>,
    class: &mut RawClass,
    state: &mut BodyState,
    spec: &mut Spec<'_>,
) -> bool {
    if cursor.is_punct('{') {
        enter_brace(state, cursor);
        return false;
    }
    if cursor.is_punct('}') {
        return close_brace(state, cursor);
    }
    if !read_body_word(cursor, class, state, spec) {
        cursor.step();
    }
    false
}

/// One `{` inside the body, opening a `States` block when one was named.
fn enter_brace(state: &mut BodyState, cursor: &mut Cursor<'_>) {
    state.depth = state.depth.saturating_add(1);
    state.states_at = state.pending_states.then_some(state.depth);
    state.pending_states = false;
    cursor.step();
}

/// One `}` inside the body; `true` when it closed the class.
fn close_brace(state: &mut BodyState, cursor: &mut Cursor<'_>) -> bool {
    if state.states_at == Some(state.depth) {
        state.states_at = None;
    }
    state.depth = state.depth.saturating_sub(1);
    cursor.step();
    state.depth == 0
}

/// One word of the body; `true` when the handler consumed tokens.
fn read_body_word(
    cursor: &mut Cursor<'_>,
    class: &mut RawClass,
    state: &mut BodyState,
    spec: &mut Spec<'_>,
) -> bool {
    let Some(word) = cursor.word() else {
        return false;
    };
    if word.eq_ignore_ascii_case("states") && state.depth == 1 {
        state.pending_states = true;
        cursor.step();
        return true;
    }
    if word.eq_ignore_ascii_case("radius") {
        return read_radius(cursor, class, spec);
    }
    let in_states = state.states_at == Some(state.depth);
    if word.eq_ignore_ascii_case("spawn") && in_states && class.sprite.is_none() {
        return read_spawn_state(cursor, class);
    }
    false
}

/// `Radius <n>`: the keyword sits under the cursor.
fn read_radius(cursor: &mut Cursor<'_>, class: &mut RawClass, spec: &mut Spec<'_>) -> bool {
    cursor.step();
    let Some(text) = cursor.number() else {
        return true;
    };
    match number_value(text) {
        Some(value) => class.radius = class.radius.or(Some(value)),
        None => spec.warn(format!("{}: ignoring radius {text:?}", spec.prefix)),
    }
    cursor.step();
    true
}

/// The first state line after the `Spawn:` label: sprite and frame letter.
fn read_spawn_state(cursor: &mut Cursor<'_>, class: &mut RawClass) -> bool {
    cursor.step();
    if cursor.is_punct(':') {
        cursor.step();
    }
    let Some(sprite) = cursor.word().filter(|word| is_sprite_name(word)) else {
        return true;
    };
    class.sprite = Some(sprite.to_owned());
    cursor.step();
    if let Some(frame) = cursor.word() {
        class.frame = frame.chars().next();
        cursor.step();
    }
    true
}

/// Whether `text` is a four-letter sprite name.
fn is_sprite_name(text: &str) -> bool {
    text.len() == SPRITE_LEN && text.bytes().all(|byte| byte.is_ascii_alphabetic())
}

/// The whole-number value of a numeric token, dropping any fraction.
fn number_value(text: &str) -> Option<u32> {
    let head = text.split_once('.').map_or(text, |(int, _)| int);
    head.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::{Kind, is_sprite_name, number_value, parse, parse_decorate, scan};

    #[test]
    fn the_scanner_drops_comments_and_strings_with_braces() {
        let text = "actor A 1 { Radius 16 /* } hidden */ // {\n \"} { braces\"\n}";
        let tokens = scan(text);
        let braces: usize = tokens
            .iter()
            .filter(|tok| tok.kind == Kind::Punct && (tok.text == "{" || tok.text == "}"))
            .count();
        assert_eq!(braces, 2, "only the real body braces remain: {tokens:?}");
        assert!(
            !tokens
                .iter()
                .any(|tok| tok.kind == Kind::Word && tok.text == "braces"),
            "string and comment contents never become tokens: {tokens:?}"
        );
    }

    #[test]
    fn an_actor_with_a_number_becomes_a_class_with_sprite_and_radius() {
        let text = "actor PlasmaLamp 20010 {\n Radius 24\n States {\n Spawn: PLSM A 10 Bright\n \
                    }\n}";
        let (classes, warnings) = parse_decorate(text);
        assert!(warnings.is_empty(), "no warnings: {warnings:?}");
        let lamp = classes.first().expect("one class");
        assert_eq!(lamp.name, "PlasmaLamp", "name");
        assert_eq!(lamp.doomednum, Some(20_010), "header doomednum");
        assert_eq!(lamp.radius, Some(24), "radius");
        assert_eq!(lamp.sprite.as_deref(), Some("PLSM"), "spawn sprite");
        assert_eq!(lamp.frame, Some('A'), "first frame letter");
    }

    #[test]
    fn parent_replaces_and_lowercase_are_all_read() {
        let text = "Actor LampBase : Inventory replaces Tool {\n radius 16.5\n states {\n \
                     Spawn: TARG B 5\n See: TARG C\n }\n}";
        let (classes, warnings) = parse_decorate(text);
        assert!(warnings.is_empty(), "no warnings: {warnings:?}");
        let base = classes.first().expect("one class");
        assert_eq!(base.parent.as_deref(), Some("Inventory"), "colon parent");
        assert_eq!(base.radius, Some(16), "the integer part of the radius");
        assert_eq!(base.sprite.as_deref(), Some("TARG"), "first spawn state");
        assert_eq!(base.frame, Some('B'), "its first frame letter");
        assert_eq!(base.doomednum, None, "no number in the header");
    }

    #[test]
    fn the_spawn_sprite_only_comes_from_the_states_block() {
        let text = "actor Foo {\n setspawnparam spawn sprite\n States {\n Greet: BRS1 A 1\n \
                    Spawn: BONU AA 1\n }\n}";
        let (classes, warnings) = parse_decorate(text);
        assert!(warnings.is_empty(), "no warnings: {warnings:?}");
        let foo = classes.first().expect("one class");
        assert_eq!(
            foo.sprite.as_deref(),
            Some("BONU"),
            "Spawn, not Greet or loose words"
        );
        assert_eq!(foo.frame, Some('A'), "the first letter of the frame");
    }

    #[test]
    fn a_class_without_a_name_warns_and_is_skipped() {
        let text = "actor { radius 8 } actor Ok 5 { }";
        let (classes, warnings) = parse_decorate(text);
        assert_eq!(
            warnings.len(),
            1,
            "the headerless class warns: {warnings:?}"
        );
        assert_eq!(classes.len(), 1, "the named one survives");
        assert_eq!(classes.first().expect("class").name, "Ok", "kept class");
    }

    #[test]
    fn an_unterminated_class_body_warns_but_keeps_what_it_saw() {
        let text = "actor Cut 30 { radius 12 states { spawn: POSS A";
        let (classes, warnings) = parse_decorate(text);
        assert_eq!(warnings.len(), 1, "the truncation warns: {warnings:?}");
        let cut = classes.first().expect("the class is kept");
        assert_eq!(cut.doomednum, Some(30), "header parsed");
        assert_eq!(cut.radius, Some(12), "radius parsed");
        assert_eq!(cut.sprite.as_deref(), Some("POSS"), "state parsed");
    }

    #[test]
    fn the_zscript_dialect_ignores_header_numbers() {
        let text = "class Foo : Actor 20010 { Default { Radius 30; } States { Spawn: FOOB A 8 } }";
        let (classes, warnings) = parse(text, "class", "zscript", false);
        assert!(warnings.is_empty(), "no warnings: {warnings:?}");
        let foo = classes.first().expect("one class");
        assert_eq!(foo.doomednum, None, "zscript numbers are not doomednums");
        assert_eq!(foo.radius, Some(30), "radius inside the Default block");
        assert_eq!(foo.sprite.as_deref(), Some("FOOB"), "states block");
    }

    #[test]
    fn helpers_cover_sprite_shapes_and_numbers() {
        assert!(is_sprite_name("POSS"), "four letters");
        assert!(!is_sprite_name("POSSS"), "too long");
        assert!(!is_sprite_name("POS1"), "digits are not letters");
        assert_eq!(number_value("16"), Some(16_u32), "plain integer");
        assert_eq!(number_value("16.5"), Some(16_u32), "fraction dropped");
        assert_eq!(number_value("x"), None, "not a number");
    }
}
