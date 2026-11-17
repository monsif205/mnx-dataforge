//! HTML text extraction: tolerant tokenizer/tree builder, boilerplate removal (navigation, footers, sidebars,
//! ads, cookie banners, menus), main-article selection, paragraph/heading reconstruction and source metadata.
//! Pure std. Handles broken markup (unclosed tags, stray '<', bad attributes, unterminated comments).

use crate::json;
use crate::nlp;
use crate::unorm::{clean, CleanOpts};

const MAX_NODES: usize = 3_000_000;
const MAX_DEPTH: usize = 400;

pub struct Node {
    pub tag: String, // "" = text node, "#root" = root
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub parent: usize,
    pub kids: Vec<usize>,
}

impl Node {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

pub struct Dom {
    pub nodes: Vec<Node>,
    pub title: String,
    pub ld_json: Vec<String>,
    pub truncated: bool,
}

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
const BLOCKS: &[&str] = &[
    "address", "article", "aside", "blockquote", "dd", "details", "div", "dl", "dt", "fieldset", "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "li", "main", "nav", "ol", "p", "pre", "section", "table",
    "tbody", "td", "tfoot", "th", "thead", "tr", "ul", "body", "html", "summary", "menu",
];
const DROP_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "iframe", "object", "embed", "svg", "canvas", "form", "select", "option", "button", "input", "textarea", "nav", "footer", "aside", "dialog", "menu", "map", "audio", "video", "head", "label", "datalist", "meter", "progress",
];
const NEG_TOKENS: &[&str] = &[
    "ad", "ads", "advert", "advertisement", "adsense", "adslot", "adsbygoogle", "banner", "cookie", "cookies", "consent", "gdpr", "onetrust", "popup", "modal", "overlay", "newsletter", "subscribe", "subscription", "sidebar", "widget", "widgets", "breadcrumb",
    "breadcrumbs", "share", "sharing", "social", "related", "recommended", "comments", "comment", "footer", "masthead", "navbar", "nav", "navigation", "menu", "toolbar", "sponsor", "sponsored", "promo", "skip", "pagination", "pager", "tooltip", "lightbox",
    "sticky", "outbrain", "taboola", "disqus", "signup",
];
const POS_TOKENS: &[&str] = &["article", "content", "post", "entry", "main", "story", "body", "text", "articlebody", "prose"];
const NEG_ROLES: &[&str] = &["navigation", "banner", "contentinfo", "complementary", "search", "dialog", "alertdialog", "menubar", "toolbar", "menu"];

fn is_block(t: &str) -> bool {
    BLOCKS.contains(&t)
}

fn closes(top: &str, new: &str) -> bool {
    (top == "p" && (is_block(new) && new != "a")) || (top == "li" && new == "li") || ((top == "dt" || top == "dd") && (new == "dt" || new == "dd")) || (top == "tr" && new == "tr")
        || ((top == "td" || top == "th") && matches!(new, "td" | "th" | "tr"))
        || (top == "option" && new == "option")
        || (matches!(top, "thead" | "tbody" | "tfoot") && matches!(new, "thead" | "tbody" | "tfoot"))
}

fn find_ci(h: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || h.len() < needle.len() {
        return None;
    }
    let mut i = from;
    while i + needle.len() <= h.len() {
        if h[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            return Some(i);
        }
        i += 1;
    }
    None
}

const ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"), ("lt", "<"), ("gt", ">"), ("quot", "\""), ("apos", "'"), ("nbsp", " "), ("copy", "©"), ("reg", "®"), ("trade", "™"), ("mdash", "—"), ("ndash", "–"), ("hellip", "…"), ("laquo", "«"), ("raquo", "»"),
    ("ldquo", "“"), ("rdquo", "”"), ("lsquo", "‘"), ("rsquo", "’"), ("bull", "•"), ("middot", "·"), ("euro", "€"), ("pound", "£"), ("yen", "¥"), ("cent", "¢"), ("deg", "°"), ("times", "×"), ("divide", "÷"), ("eacute", "é"),
    ("egrave", "è"), ("ecirc", "ê"), ("agrave", "à"), ("acirc", "â"), ("ccedil", "ç"), ("ocirc", "ô"), ("ucirc", "û"), ("ugrave", "ù"), ("icirc", "î"), ("iuml", "ï"), ("euml", "ë"), ("auml", "ä"), ("ouml", "ö"), ("uuml", "ü"),
    ("szlig", "ß"), ("ntilde", "ñ"), ("aacute", "á"), ("iacute", "í"), ("oacute", "ó"), ("uacute", "ú"), ("sect", "§"), ("para", "¶"), ("shy", ""), ("zwj", ""), ("zwnj", ""), ("rlm", ""), ("lrm", ""), ("ensp", " "), ("emsp", " "), ("thinsp", " "),
];

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        if b[i] == b'&' {
            let mut j = i + 1;
            while j < b.len() && j - i <= 12 && (b[j].is_ascii_alphanumeric() || b[j] == b'#') {
                j += 1;
            }
            let name = &s[i + 1..j];
            let semi = j < b.len() && b[j] == b';';
            let rep: Option<String> = if let Some(num) = name.strip_prefix('#') {
                let v = if let Some(h) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) { u32::from_str_radix(h, 16).ok() } else { num.parse::<u32>().ok() };
                v.and_then(char::from_u32).map(|c| c.to_string())
            } else if semi {
                ENTITIES.iter().find(|(n, _)| *n == name).map(|(_, v)| v.to_string())
            } else {
                None
            };
            if let Some(r) = rep {
                out.push_str(&s[last..i]);
                out.push_str(&r);
                i = if semi { j + 1 } else { j };
                last = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

pub fn parse(html: &str) -> Dom {
    let b = html.as_bytes();
    let mut nodes = vec![Node { tag: "#root".into(), attrs: vec![], text: String::new(), parent: 0, kids: vec![] }];
    let mut stack: Vec<usize> = vec![0];
    let mut title = String::new();
    let mut ld = Vec::new();
    let mut truncated = false;
    let mut i = 0usize;
    let mut text_start = 0usize;

    macro_rules! flush_text {
        ($end:expr) => {
            if $end > text_start {
                let t = decode_entities(&html[text_start..$end]);
                if !t.is_empty() && nodes.len() < MAX_NODES {
                    let parent = *stack.last().unwrap();
                    let id = nodes.len();
                    nodes.push(Node { tag: String::new(), attrs: vec![], text: t, parent, kids: vec![] });
                    nodes[parent].kids.push(id);
                }
            }
        };
    }

    while i < b.len() {
        if nodes.len() >= MAX_NODES {
            truncated = true;
            break;
        }
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &b[i + 1..];
        if rest.starts_with(b"!--") {
            flush_text!(i);
            i = match find_ci(b, b"-->", i + 4) {
                Some(e) => e + 3,
                None => b.len(),
            };
            text_start = i;
            continue;
        }
        if rest.starts_with(b"![CDATA[") {
            flush_text!(i);
            let end = find_ci(b, b"]]>", i + 9).unwrap_or(b.len());
            text_start = i + 9;
            flush_text!(end);
            i = (end + 3).min(b.len());
            text_start = i;
            continue;
        }
        if rest.first().map(|c| *c == b'!' || *c == b'?').unwrap_or(false) {
            flush_text!(i);
            i = b[i..].iter().position(|&c| c == b'>').map(|p| i + p + 1).unwrap_or(b.len());
            text_start = i;
            continue;
        }
        if rest.first() == Some(&b'/') {
            let mut j = i + 2;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b':' || b[j] == b'-') {
                j += 1;
            }
            if j == i + 2 {
                i += 1;
                continue; // "</" followed by junk is literal text
            }
            flush_text!(i);
            let name = html[i + 2..j].to_ascii_lowercase();
            if let Some(pos) = stack.iter().rposition(|&n| nodes[n].tag == name) {
                if pos > 0 {
                    stack.truncate(pos);
                }
            }
            i = b[j..].iter().position(|&c| c == b'>').map(|p| j + p + 1).unwrap_or(b.len());
            text_start = i;
            continue;
        }
        if !rest.first().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
            i += 1; // stray '<'
            continue;
        }
        // start tag
        let mut j = i + 1;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b':' || b[j] == b'-' || b[j] == b'_') {
            j += 1;
        }
        let tag = html[i + 1..j].to_ascii_lowercase();
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut self_close = false;
        loop {
            while j < b.len() && (b[j].is_ascii_whitespace() || b[j] == b'/') {
                if b[j] == b'/' {
                    self_close = true;
                }
                j += 1;
            }
            if j >= b.len() {
                break;
            }
            if b[j] == b'>' {
                j += 1;
                break;
            }
            if b[j] == b'<' {
                break; // broken tag: the next tag starts here
            }
            self_close = false;
            let ns = j;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'=' && b[j] != b'>' && b[j] != b'/' && b[j] != b'<' {
                j += 1;
            }
            if j == ns {
                j += 1;
                continue;
            }
            let aname = html[ns..j].to_ascii_lowercase();
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let mut val = String::new();
            if j < b.len() && b[j] == b'=' {
                j += 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                    let q = b[j];
                    let vs = j + 1;
                    let mut e = vs;
                    while e < b.len() && b[e] != q {
                        e += 1;
                    }
                    val = decode_entities(&html[vs..e]);
                    j = (e + 1).min(b.len());
                } else {
                    let vs = j;
                    while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                        j += 1;
                    }
                    val = decode_entities(&html[vs..j]);
                }
            }
            if attrs.len() < 64 {
                attrs.push((aname, val));
            }
        }
        flush_text!(i);
        let parent_before = *stack.last().unwrap();
        while stack.len() > 1 && closes(&nodes[*stack.last().unwrap()].tag, &tag) {
            stack.pop();
        }
        let parent = *stack.last().unwrap();
        let _ = parent_before;
        let id = nodes.len();
        nodes.push(Node { tag: tag.clone(), attrs, text: String::new(), parent, kids: vec![] });
        nodes[parent].kids.push(id);
        i = j;
        text_start = i;
        if tag == "script" || tag == "style" || tag == "title" || tag == "textarea" {
            let close = format!("</{}", tag);
            let end = find_ci(b, close.as_bytes(), i).unwrap_or(b.len());
            let content = &html[i..end];
            if tag == "title" && title.is_empty() {
                title = decode_entities(content).trim().to_string();
            } else if tag == "script" && nodes[id].attr("type").map(|t| t.contains("ld+json")).unwrap_or(false) {
                ld.push(content.to_string());
            }
            i = b[end..].iter().position(|&c| c == b'>').map(|p| end + p + 1).unwrap_or(b.len());
            text_start = i;
            continue;
        }
        if !(VOID.contains(&tag.as_str()) || self_close) && stack.len() < MAX_DEPTH {
            stack.push(id);
        }
    }
    flush_text!(b.len().max(text_start));
    Dom { nodes, title, ld_json: ld, truncated }
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| t.to_lowercase()).collect()
}

fn self_boilerplate(n: &Node) -> bool {
    if DROP_TAGS.contains(&n.tag.as_str()) {
        return true;
    }
    if n.attr("hidden").is_some() || n.attr("aria-hidden") == Some("true") {
        return true;
    }
    if let Some(st) = n.attr("style") {
        let s: String = st.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
        if s.contains("display:none") || s.contains("visibility:hidden") {
            return true;
        }
    }
    if let Some(r) = n.attr("role") {
        if NEG_ROLES.contains(&r) {
            return true;
        }
    }
    if matches!(n.tag.as_str(), "html" | "body" | "article" | "main") || n.tag.is_empty() {
        return false;
    }
    let mut toks = tokens(n.attr("class").unwrap_or(""));
    toks.extend(tokens(n.attr("id").unwrap_or("")));
    if toks.is_empty() {
        return false;
    }
    let neg = toks.iter().any(|t| NEG_TOKENS.contains(&t.as_str()));
    let pos = toks.iter().any(|t| POS_TOKENS.contains(&t.as_str()));
    neg && !pos
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Heading,
    Para,
    Item,
    Code,
    Quote,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: Kind,
    pub level: u8,
    pub text: String,
}

pub struct Opts {
    pub markdown: bool,
    pub min_block_chars: usize,
    pub clean: CleanOpts,
    pub keep_tables: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { markdown: true, min_block_chars: 0, clean: CleanOpts::default(), keep_tables: true }
    }
}

pub struct Doc {
    pub title: String,
    pub lang: String,
    pub blocks: Vec<Block>,
    pub text: String,
    pub meta: Vec<(String, String)>,
    pub license_hints: Vec<String>,
    pub raw_chars: usize,
    pub kept_chars: usize,
    pub removed_blocks: usize,
    pub truncated: bool,
}

const JUNK_PHRASES: &[&str] = &[
    "share this", "share on", "read more", "continue reading", "click here", "subscribe", "sign up", "sign in", "log in", "accept all", "accept cookies", "we use cookies", "all rights reserved", "advertisement", "sponsored", "skip to content",
    "privacy policy", "terms of use", "terms of service", "cookie policy", "follow us", "related articles", "you may also like", "back to top", "tous droits réservés", "lire la suite", "politique de confidentialité", "accepter les cookies",
    "todos los derechos reservados", "leer más", "alle rechte vorbehalten", "اقرأ المزيد", "جميع الحقوق محفوظة", "سياسة الخصوصية", "شارك", "اشترك", "تابعنا", "إعلان", "الإعلانات", "تسجيل الدخول",
];

fn is_junk_line(t: &str) -> bool {
    if t.chars().count() > 110 {
        return false;
    }
    let l = t.to_lowercase();
    JUNK_PHRASES.iter().any(|p| l.contains(p))
}

struct Ctx<'a> {
    d: &'a Dom,
    pruned: Vec<bool>,
    tl: Vec<usize>,
    ll: Vec<usize>,
}

fn analyse(d: &Dom) -> Ctx<'_> {
    let n = d.nodes.len();
    let mut pruned = vec![false; n];
    let mut in_link = vec![false; n];
    let mut in_article = vec![false; n];
    for i in 1..n {
        let nd = &d.nodes[i];
        let p = nd.parent;
        in_article[i] = in_article[p] || nd.tag == "article" || nd.tag == "main";
        in_link[i] = in_link[p] || nd.tag == "a";
        let mut pr = pruned[p] || self_boilerplate(nd);
        if nd.tag == "header" && !in_article[i] {
            pr = true;
        }
        pruned[i] = pr;
    }
    let mut tl = vec![0usize; n];
    let mut ll = vec![0usize; n];
    for i in (1..n).rev() {
        let nd = &d.nodes[i];
        if nd.tag.is_empty() {
            if !pruned[i] {
                let c = nd.text.split_whitespace().map(|w| w.chars().count() + 1).sum::<usize>();
                tl[i] += c;
                if in_link[i] {
                    ll[i] += c;
                }
            }
        }
        let p = nd.parent;
        if p != i {
            tl[p] += tl[i];
            ll[p] += ll[i];
        }
    }
    Ctx { d, pruned, tl, ll }
}

fn pick_container(c: &Ctx) -> usize {
    let d = c.d;
    let n = d.nodes.len();
    let mut score = vec![0f64; n];
    for i in 1..n {
        let nd = &d.nodes[i];
        if c.pruned[i] || !matches!(nd.tag.as_str(), "p" | "pre" | "blockquote" | "li" | "td") {
            continue;
        }
        let len = c.tl[i];
        if len < 25 {
            continue;
        }
        let commas = nd.kids.iter().filter(|&&k| d.nodes[k].tag.is_empty()).map(|&k| d.nodes[k].text.matches([',', '،', '，']).count()).sum::<usize>();
        let s = (len.min(300) as f64) / 50.0 + 1.0 + commas as f64;
        let p1 = nd.parent;
        score[p1] += s;
        let p2 = d.nodes[p1].parent;
        if p2 != p1 {
            score[p2] += s / 2.0;
            let p3 = d.nodes[p2].parent;
            if p3 != p2 {
                score[p3] += s / 3.0;
            }
        }
    }
    let mut best = 0usize;
    let mut best_s = 0f64;
    for i in 1..n {
        let nd = &d.nodes[i];
        if c.pruned[i] || !matches!(nd.tag.as_str(), "div" | "section" | "article" | "main" | "td" | "body" | "ul" | "ol" | "dl") {
            continue;
        }
        let ld = if c.tl[i] > 0 { c.ll[i] as f64 / c.tl[i] as f64 } else { 1.0 };
        let s = score[i] * (1.0 - ld.min(0.95));
        if s > best_s {
            best_s = s;
            best = i;
        }
    }
    // semantic hint: a real <article>/<main> that holds most of the text wins
    let mut sem = 0usize;
    for i in 1..n {
        let nd = &d.nodes[i];
        let semantic = nd.tag == "article" || nd.tag == "main" || nd.attr("role") == Some("main") || nd.attr("itemprop") == Some("articleBody");
        if semantic && !c.pruned[i] && (sem == 0 || c.tl[i] > c.tl[sem]) {
            sem = i;
        }
    }
    if sem != 0 && (best == 0 || c.tl[sem] * 10 >= c.tl[best] * 6) {
        return sem;
    }
    if best != 0 {
        return best;
    }
    d.nodes.iter().position(|n| n.tag == "body").unwrap_or(0)
}

fn inline_text(c: &Ctx, i: usize, out: &mut String) {
    let nd = &c.d.nodes[i];
    if c.pruned[i] {
        return;
    }
    if nd.tag.is_empty() {
        out.push_str(&nd.text);
        return;
    }
    if nd.tag == "br" {
        out.push('\n');
        return;
    }
    if nd.tag == "img" {
        return;
    }
    for &k in &nd.kids {
        inline_text(c, k, out);
    }
}

struct Walker<'a> {
    c: &'a Ctx<'a>,
    blocks: Vec<Block>,
    removed: usize,
    keep_tables: bool,
}

impl Walker<'_> {
    fn emit(&mut self, kind: Kind, level: u8, raw: &str) {
        let text = if kind == Kind::Code { raw.trim_matches('\n').to_string() } else { reflow_br(raw) };
        if text.trim().is_empty() {
            return;
        }
        self.blocks.push(Block { kind, level, text });
    }

    fn list_is_nav(&self, i: usize) -> bool {
        let c = self.c;
        let items = c.d.nodes[i].kids.iter().filter(|&&k| matches!(c.d.nodes[k].tag.as_str(), "li" | "dt" | "dd")).count().max(1);
        c.tl[i] > 0 && c.ll[i] as f64 / c.tl[i] as f64 > 0.6 && c.tl[i] / items < 80
    }

    fn walk(&mut self, i: usize, ordered_idx: &mut usize) {
        let c = self.c;
        let nd = &c.d.nodes[i];
        if c.pruned[i] {
            if c.tl.get(i).copied().unwrap_or(0) > 0 {
                self.removed += 1;
            }
            return;
        }
        let tag = nd.tag.as_str();
        match tag {
            "" | "br" | "img" => {}
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let mut s = String::new();
                inline_text(c, i, &mut s);
                self.emit(Kind::Heading, tag.as_bytes()[1] - b'0', &s);
                return;
            }
            "p" | "figcaption" | "summary" | "dt" | "dd" | "caption" => {
                // paragraphs may contain block children in broken markup: fall through to generic handling when they do
                if !nd.kids.iter().any(|&k| is_block(&c.d.nodes[k].tag)) {
                    let mut s = String::new();
                    inline_text(c, i, &mut s);
                    self.emit(Kind::Para, 0, &s);
                    return;
                }
            }
            "pre" => {
                let mut s = String::new();
                inline_text(c, i, &mut s);
                self.emit(Kind::Code, 0, &s);
                return;
            }
            "blockquote" => {
                if !nd.kids.iter().any(|&k| is_block(&c.d.nodes[k].tag)) {
                    let mut s = String::new();
                    inline_text(c, i, &mut s);
                    self.emit(Kind::Quote, 0, &s);
                    return;
                }
            }
            "ul" | "ol" | "dl" => {
                if self.list_is_nav(i) {
                    self.removed += 1;
                    return;
                }
                let mut idx = 0usize;
                for &k in &nd.kids {
                    self.walk_list_child(k, tag == "ol", &mut idx);
                }
                return;
            }
            "table" => {
                if c.tl[i] > 0 && c.ll[i] as f64 / c.tl[i] as f64 > 0.5 {
                    self.removed += 1;
                    return;
                }
                if !self.keep_tables {
                    return;
                }
                self.table_rows(i);
                return;
            }
            _ => {}
        }
        // generic container or inline run: collect direct inline content between block children
        let mut run = String::new();
        let mut run_has = false;
        for &k in &nd.kids {
            let kn = &c.d.nodes[k];
            if kn.tag.is_empty() || (!is_block(&kn.tag) && !matches!(kn.tag.as_str(), "ul" | "ol" | "dl" | "table" | "pre" | "blockquote")) {
                if !c.pruned[k] {
                    inline_text(c, k, &mut run);
                    run_has = true;
                }
            } else {
                if run_has {
                    self.emit(Kind::Para, 0, &run);
                    run.clear();
                    run_has = false;
                }
                self.walk(k, ordered_idx);
            }
        }
        if run_has {
            self.emit(Kind::Para, 0, &run);
        }
    }

    fn walk_list_child(&mut self, k: usize, ordered: bool, idx: &mut usize) {
        let c = self.c;
        let kn = &c.d.nodes[k];
        if c.pruned[k] {
            return;
        }
        if kn.tag == "li" || kn.tag == "dd" || kn.tag == "dt" {
            *idx += 1;
            let mut s = String::new();
            let mut nested: Vec<usize> = Vec::new();
            for &g in &kn.kids {
                if matches!(c.d.nodes[g].tag.as_str(), "ul" | "ol") {
                    nested.push(g);
                } else {
                    inline_text(c, g, &mut s);
                }
            }
            let prefix = if ordered { format!("{}\u{1}", idx) } else { String::new() };
            let t = format!("{}{}", prefix, s);
            self.emit(Kind::Item, if ordered { 1 } else { 0 }, &t);
            for g in nested {
                let mut z = 0usize;
                self.walk(g, &mut z);
            }
        } else {
            let mut z = 0usize;
            self.walk(k, &mut z);
        }
    }

    fn table_rows(&mut self, i: usize) {
        let c = self.c;
        let mut rows: Vec<usize> = Vec::new();
        let mut stack = vec![i];
        while let Some(x) = stack.pop() {
            for &k in c.d.nodes[x].kids.iter().rev() {
                match c.d.nodes[k].tag.as_str() {
                    "tr" => rows.push(k),
                    "thead" | "tbody" | "tfoot" => stack.push(k),
                    _ => {}
                }
            }
        }
        rows.sort_unstable();
        for r in rows {
            let mut cells: Vec<String> = Vec::new();
            for &k in &c.d.nodes[r].kids {
                if matches!(c.d.nodes[k].tag.as_str(), "td" | "th") && !c.pruned[k] {
                    let mut s = String::new();
                    inline_text(c, k, &mut s);
                    cells.push(s.split_whitespace().collect::<Vec<_>>().join(" "));
                }
            }
            if cells.iter().any(|x| !x.is_empty()) {
                let line = cells.join(" | ");
                self.emit(Kind::Para, 0, &line);
            }
        }
    }
}

/// A <br> inside a paragraph is a line break only when the previous line ended a sentence; otherwise the text
/// was hard-wrapped and is joined with a space.
fn reflow_br(raw: &str) -> String {
    if !raw.contains('\n') {
        return raw.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    let lines: Vec<String> = raw.split('\n').map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect();
    let mut out = String::new();
    for (k, l) in lines.iter().enumerate() {
        if k > 0 {
            let prev = out.chars().last().unwrap_or(' ');
            out.push(if matches!(prev, '.' | '!' | '?' | '…' | '؟' | '。' | ':') { '\n' } else { ' ' });
        }
        out.push_str(l);
    }
    out
}

fn meta_pairs(d: &Dom) -> (Vec<(String, String)>, Vec<String>, String) {
    let mut meta: Vec<(String, String)> = Vec::new();
    let mut lic: Vec<String> = Vec::new();
    let mut lang = String::new();
    let mut add = |k: &str, v: &str| {
        let v = v.trim();
        if !v.is_empty() && !meta.iter().any(|(a, b)| a == k && b == v) && meta.len() < 80 {
            meta.push((k.to_string(), v.chars().take(500).collect()));
        }
    };
    for n in &d.nodes {
        match n.tag.as_str() {
            "html" => {
                if let Some(l) = n.attr("lang").or_else(|| n.attr("xml:lang")) {
                    lang = l.to_lowercase();
                }
            }
            "meta" => {
                let key = n.attr("property").or_else(|| n.attr("name")).or_else(|| n.attr("itemprop")).or_else(|| n.attr("http-equiv")).unwrap_or("").to_lowercase();
                let val = n.attr("content").unwrap_or("");
                match key.as_str() {
                    "description" | "author" | "keywords" | "robots" | "generator" | "og:title" | "og:site_name" | "og:type" | "og:url" | "og:locale" | "article:published_time" | "article:modified_time" | "article:author"
                    | "datepublished" | "datemodified" | "date" | "dc.date" | "dc.creator" | "dc.language" | "dc.publisher" | "content-language" | "twitter:site" => add(&key, val),
                    "license" | "dc.rights" | "dc.license" | "rights" | "copyright" | "dcterms.license" | "dcterms.rights" => {
                        add(&key, val);
                        lic.push(val.to_string());
                    }
                    _ => {}
                }
            }
            "link" => {
                let rel = n.attr("rel").unwrap_or("").to_lowercase();
                let href = n.attr("href").unwrap_or("");
                if rel.split_whitespace().any(|r| r == "canonical") {
                    add("canonical", href);
                }
                if rel.split_whitespace().any(|r| r == "license") {
                    add("license_link", href);
                    lic.push(href.to_string());
                }
            }
            "a" => {
                let rel = n.attr("rel").unwrap_or("").to_lowercase();
                if rel.split_whitespace().any(|r| r == "license") {
                    let href = n.attr("href").unwrap_or("");
                    add("license_link", href);
                    lic.push(href.to_string());
                }
            }
            _ => {}
        }
    }
    for raw in &d.ld_json {
        if let Ok(v) = json::parse(raw) {
            let mut items: Vec<&json::Value> = Vec::new();
            match &v {
                json::Value::Arr(a) => items.extend(a.iter()),
                other => {
                    items.push(other);
                    if let Some(g) = other.get("@graph").and_then(|g| g.as_arr()) {
                        items.extend(g.iter());
                    }
                }
            }
            for it in items {
                for k in ["headline", "datePublished", "dateModified", "inLanguage", "isAccessibleForFree"] {
                    if let Some(x) = it.get(k) {
                        let s = x.as_str().map(|s| s.to_string()).unwrap_or_else(|| x.to_json());
                        add(&format!("ld:{}", k), &s);
                    }
                }
                if let Some(a) = it.get("author") {
                    let name = a.get("name").and_then(|x| x.as_str()).or_else(|| a.as_str()).or_else(|| a.as_arr().and_then(|l| l.first()).and_then(|f| f.get("name")).and_then(|x| x.as_str()));
                    if let Some(nm) = name {
                        add("ld:author", nm);
                    }
                }
                if let Some(l) = it.get("license") {
                    let s = l.as_str().map(|s| s.to_string()).unwrap_or_else(|| l.to_json());
                    add("ld:license", &s);
                    lic.push(s);
                }
            }
        }
    }
    lic.sort();
    lic.dedup();
    lic.retain(|l| !l.trim().is_empty());
    (meta, lic, lang)
}

fn clean_title(t: &str) -> String {
    for sep in [" | ", " - ", " – ", " — ", " :: ", " » "] {
        if let Some((a, b)) = t.rsplit_once(sep) {
            if b.chars().count() < a.chars().count() && b.chars().count() <= 40 {
                return a.trim().to_string();
            }
        }
    }
    t.trim().to_string()
}

pub fn extract(html: &str, o: &Opts) -> Doc {
    let dom = parse(html);
    let ctx = analyse(&dom);
    let raw_chars = ctx.tl.get(0).copied().unwrap_or(0).max(dom.nodes.iter().filter(|n| n.tag.is_empty()).map(|n| n.text.len()).sum::<usize>().min(usize::MAX / 2) / 4);
    let root = pick_container(&ctx);
    let mut w = Walker { c: &ctx, blocks: Vec::new(), removed: 0, keep_tables: o.keep_tables };
    let mut idx = 0usize;
    if root == 0 {
        w.walk(0, &mut idx);
    } else {
        // the container itself may be an <article> with a pruned-looking header; walk its children as a container
        w.walk(root, &mut idx);
    }
    let mut removed = w.removed + (1..dom.nodes.len()).filter(|&i| ctx.pruned[i] && !ctx.pruned[dom.nodes[i].parent] && !dom.nodes[i].tag.is_empty()).count();
    let mut blocks: Vec<Block> = Vec::new();
    let co = &o.clean;
    for b in w.blocks {
        let mut t = if b.kind == Kind::Code { clean(&b.text, &CleanOpts { collapse_ws: false, ..co.clone() }) } else { clean(&b.text, co) };
        let mut num_prefix = String::new();
        if b.kind == Kind::Item {
            if let Some((n, rest)) = t.split_once('\u{1}') {
                num_prefix = format!("{}. ", n);
                t = rest.trim().to_string();
            }
        }
        if t.trim().is_empty() || !t.chars().any(|c| c.is_alphanumeric()) {
            removed += 1;
            continue;
        }
        if b.kind != Kind::Code && b.kind != Kind::Heading && is_junk_line(&t) {
            removed += 1;
            continue;
        }
        if b.kind == Kind::Para && t.chars().count() < o.min_block_chars {
            removed += 1;
            continue;
        }
        if blocks.last().map(|p: &Block| p.text == t && p.kind == b.kind).unwrap_or(false) {
            removed += 1;
            continue;
        }
        blocks.push(Block { kind: b.kind, level: b.level, text: format!("{}{}", num_prefix, t) });
    }
    let (meta, license_hints, lang) = meta_pairs(&dom);
    let title = blocks.iter().find(|b| b.kind == Kind::Heading && b.level == 1).map(|b| b.text.clone()).unwrap_or_else(|| clean_title(&clean(&dom.title, co)));
    let mut text = String::new();
    for (k, b) in blocks.iter().enumerate() {
        if k > 0 {
            text.push_str("\n\n");
        }
        match b.kind {
            Kind::Heading if o.markdown => {
                for _ in 0..b.level.max(1) {
                    text.push('#');
                }
                text.push(' ');
                text.push_str(&b.text);
            }
            Kind::Item if o.markdown => {
                if b.text.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && b.text.contains(". ") && b.level == 1 {
                    text.push_str(&b.text);
                } else {
                    text.push_str("- ");
                    text.push_str(&b.text);
                }
            }
            Kind::Code if o.markdown => {
                text.push_str("```\n");
                text.push_str(&b.text);
                text.push_str("\n```");
            }
            Kind::Quote if o.markdown => {
                text.push_str("> ");
                text.push_str(&b.text.replace('\n', "\n> "));
            }
            _ => text.push_str(&b.text),
        }
    }
    let kept_chars = text.chars().count();
    let lang = if lang.is_empty() { nlp::detect_lang(&text).0 } else { lang };
    Doc { title, lang, blocks, text, meta, license_hints, raw_chars, kept_chars, removed_blocks: removed, truncated: dom.truncated }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!DOCTYPE html><html lang="en"><head><title>Great Post | My Site</title>
<meta name="author" content="Jane Doe"><meta property="article:published_time" content="2024-05-01T10:00:00Z">
<link rel="license" href="https://creativecommons.org/licenses/by/4.0/">
<script>var x = "<p>not content</p>";</script><style>.a{color:red}</style></head>
<body><header class="site-header"><a href="/">Home</a> <a href="/about">About</a></header>
<nav><ul><li><a href="/a">Alpha</a></li><li><a href="/b">Beta</a></li></ul></nav>
<div class="cookie-banner">We use cookies. <button>Accept</button></div>
<div class="sidebar"><h3>Popular</h3><ul><li><a href="/1">One</a></li><li><a href="/2">Two</a></li></ul></div>
<article><h1>Great Post</h1>
<p>The first paragraph has enough words, with commas, to count as real content for the extractor.
<p>The second paragraph is <b>bold</b> and &amp; has entities &eacute; &#233; &#x41;.
<h2>A heading</h2>
<ul><li>First point is a real sentence about things<li>Second point also has text</ul>
<pre>line1
  indented</pre>
<div class="ad-slot">Buy now! Advertisement</div>
<p>Broken <a href="x">link text <p>third paragraph after broken markup, still content here.
</article>
<div class="comments"><p>Great article! Thanks for sharing</p></div>
<footer>Copyright 2024. All rights reserved. <a href="/privacy">Privacy policy</a></footer>
<!-- unterminated comment at the end"#;

    #[test]
    fn extracts_main_article() {
        let d = extract(PAGE, &Opts::default());
        assert_eq!(d.title, "Great Post");
        assert_eq!(d.lang, "en");
        let t = &d.text;
        assert!(t.contains("# Great Post"), "{}", t);
        assert!(t.contains("The first paragraph has enough words"));
        assert!(t.contains("bold and & has entities é é A."), "{}", t);
        assert!(t.contains("## A heading"));
        assert!(t.contains("- First point is a real sentence"));
        assert!(t.contains("- Second point also has text"));
        assert!(t.contains("```\nline1\n  indented\n```"), "{}", t);
        assert!(t.contains("third paragraph after broken markup"));
        for gone in ["Home", "Alpha", "cookies", "Popular", "Buy now", "Thanks for sharing", "All rights reserved", "not content", "color:red"] {
            assert!(!t.contains(gone), "leaked {:?} in {}", gone, t);
        }
        assert!(d.meta.iter().any(|(k, v)| k == "author" && v == "Jane Doe"));
        assert!(d.meta.iter().any(|(k, _)| k == "article:published_time"));
        assert_eq!(d.license_hints, vec!["https://creativecommons.org/licenses/by/4.0/".to_string()]);
        assert!(d.removed_blocks > 0);
    }

    #[test]
    fn scores_content_without_semantic_tags() {
        let page = "<html><body><div id='top'><a href='/'>Home</a> <a href='/x'>X</a></div><div class='wrap'><div class='box'><p>This is a long paragraph of the real story, with several clauses, commas, and plenty of text to score.</p><p>Another long paragraph follows here, continuing the story with more words, more commas, and more detail.</p></div></div><div id='links'><a href='1'>a</a><a href='2'>b</a><a href='3'>c</a></div></body></html>";
        let d = extract(page, &Opts::default());
        assert!(d.text.contains("real story") && d.text.contains("Another long paragraph"));
        assert!(!d.text.contains("Home"));
    }

    #[test]
    fn survives_garbage_and_arabic() {
        let d = extract("<p>unclosed <b>bold <i>x</p><div<<>> text & stray < sign </div><p dir=rtl>مرحبا&nbsp;بالعالم. هذه جملة ثانية.", &Opts::default());
        assert!(d.text.contains("مرحبا بالعالم"));
        let d = extract("<p dir=rtl>مرحبا&nbsp;بالعالم. هذه جملة ثانية وطويلة قليلا.</p>", &Opts::default());
        assert_eq!(d.lang, "ar");
        let d = extract("", &Opts::default());
        assert!(d.blocks.is_empty());
        let deep = "<div>".repeat(5000) + "<p>deep text content that is long enough to keep around</p>";
        let d = extract(&deep, &Opts::default());
        assert!(d.text.contains("deep text content"));
    }

    #[test]
    fn nav_like_lists_are_suppressed() {
        let page = "<body><main><p>Real content paragraph that is long enough to be considered, with commas, and more words.</p><ul><li><a href='1'>Link one</a></li><li><a href='2'>Link two</a></li><li><a href='3'>Link three</a></li></ul></main></body>";
        let d = extract(page, &Opts::default());
        assert!(!d.text.contains("Link one"));
        assert!(d.text.contains("Real content"));
    }
}
