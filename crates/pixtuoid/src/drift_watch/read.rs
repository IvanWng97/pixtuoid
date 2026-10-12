//! The readers: how a document yields upstream's names, one per
//! [`Read`](pixtuoid_core::source::upstream::Read) variant. A reader that
//! cannot find its declaration says what it found instead, which the watch
//! files as probe health, never as a rename. Source is parsed by the
//! language's own parser (syn, oxc, pulldown-cmark); a document it rejects is
//! probe health too, and a partial tree is never read.

use std::collections::BTreeSet;

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_ast::ast::{
    ArrayExpressionElement, BindingPattern, ClassElement, Declaration, Expression,
    MethodDefinitionKind, ModuleDeclaration, PropertyKey, ReturnStatement, Statement, TSLiteral,
    TSNamespaceDeclarationBody, TSPropertySignature, TSSignature, TSType, VariableDeclarationKind,
};
use oxc_ast_visit::Visit as _;
use oxc_span::GetSpan as _;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde_json::Value;
use syn::spanned::Spanned;
use syn::visit::Visit;

pub(super) type Names = BTreeSet<String>;

/// Why a reader yields no names.
#[derive(Debug)]
pub(super) enum Unread {
    /// A pattern or item spec of ours does not compile.
    Ours(String),
    /// The document lacks what the reader reads: this is what it found
    /// instead, worded to follow "the reader found".
    Missing(String),
}

impl From<regex::Error> for Unread {
    fn from(e: regex::Error) -> Self {
        Self::Ours(e.to_string())
    }
}

fn missing(what: impl Into<String>) -> Unread {
    Unread::Missing(what.into())
}

/// A declaration's names, and why some may be out of the read's sight: one
/// declared in a base type or behind a computed key is no evidence it vanished.
pub(super) struct Declared {
    pub(super) names: Names,
    pub(super) partial: Option<String>,
}

impl From<Names> for Declared {
    fn from(names: Names) -> Self {
        Self {
            names,
            partial: None,
        }
    }
}

/// Whether `name` is a field, not one of a `TsFields` read's facts beside it.
pub(super) fn is_bare(name: &str) -> bool {
    field_of(name) == name
}

/// The field a `TsFields` fact is about: itself for a field.
pub(super) fn field_of(name: &str) -> &str {
    match name.split_once(":\"") {
        Some((field, _)) => field,
        None => name.strip_suffix('?').unwrap_or(name),
    }
}

fn joined(why: BTreeSet<String>) -> Option<String> {
    (!why.is_empty()).then(|| why.into_iter().collect::<Vec<_>>().join("; "))
}

/// The ONE item found: a first-match read is silently wrong the day upstream
/// grows a second declaration above the real one (a `#[cfg(test)]` twin, a
/// second invocation of the macro), so none or several is probe health.
fn one<T>(mut found: impl Iterator<Item = T>, what: &str) -> Result<T, Unread> {
    match (found.next(), found.next()) {
        (Some(it), None) => Ok(it),
        (None, _) => Err(missing(format!("no {what}"))),
        (Some(_), Some(_)) => Err(missing(format!("more than one {what}"))),
    }
}

/// The ones of `ours` that `pattern` finds, `{name}` standing for each.
pub(super) fn each(text: &str, pattern: &str, ours: &BTreeSet<&str>) -> Result<Names, Unread> {
    let mut found = Names::new();
    for name in ours {
        if Regex::new(&pattern.replace("{name}", &regex::escape(name)))?.is_match(text) {
            found.insert((*name).to_owned());
        }
    }
    Ok(found)
}

pub(super) fn matches(text: &str, pattern: &str) -> Result<Names, Unread> {
    let found: Names = Regex::new(pattern)?
        .captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned()))
        .collect();
    if found.is_empty() {
        return Err(missing("no match of its pattern"));
    }
    Ok(found)
}

fn rust_file(text: &str) -> Result<syn::File, Unread> {
    syn::parse_file(text).map_err(|e| missing(format!("Rust that syn cannot parse ({e})")))
}

/// Every declaration of a Rust file, however deeply nested: a module, an impl
/// or a fn body may hold the one a reader wants.
#[derive(Default)]
struct RustItems<'ast> {
    enums: Vec<&'ast syn::ItemEnum>,
    structs: Vec<&'ast syn::ItemStruct>,
    consts: Vec<(&'ast syn::Ident, &'ast syn::Expr)>,
    impls: Vec<&'ast syn::ItemImpl>,
    fns: Vec<(&'ast syn::Ident, &'ast syn::Block)>,
    macros: Vec<&'ast syn::Macro>,
}

impl<'ast> RustItems<'ast> {
    fn of(file: &'ast syn::File) -> Self {
        let mut items = Self::default();
        items.visit_file(file);
        items
    }
}

impl<'ast> Visit<'ast> for RustItems<'ast> {
    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.enums.push(i);
        syn::visit::visit_item_enum(self, i);
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.structs.push(i);
        syn::visit::visit_item_struct(self, i);
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.consts.push((&i.ident, &i.expr));
        syn::visit::visit_item_const(self, i);
    }

    fn visit_impl_item_const(&mut self, i: &'ast syn::ImplItemConst) {
        self.consts.push((&i.ident, &i.expr));
        syn::visit::visit_impl_item_const(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        self.impls.push(i);
        syn::visit::visit_item_impl(self, i);
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.fns.push((&i.sig.ident, &i.block));
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.fns.push((&i.sig.ident, &i.block));
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        if let Some(block) = &i.default {
            self.fns.push((&i.sig.ident, block));
        }
        syn::visit::visit_trait_item_fn(self, i);
    }

    fn visit_macro(&mut self, i: &'ast syn::Macro) {
        self.macros.push(i);
        syn::visit::visit_macro(self, i);
    }
}

/// The value `key` takes in an item's `#[serde(...)]`, as a string literal.
/// Only `key = "…"` counts: `rename` alone, never an `alias`, which serde only
/// ever reads, so a name moved into one is gone from the wire.
fn serde_value(attrs: &[syn::Attribute], key: &str) -> Result<Option<String>, Unread> {
    let mut found = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        let metas = attr
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .map_err(|e| missing(format!("an unreadable `#[serde]` attribute ({e})")))?;
        for meta in metas {
            match meta {
                syn::Meta::NameValue(nv) if nv.path.is_ident(key) => {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = nv.value
                    {
                        found = Some(s.value());
                    }
                }
                // `rename(serialize = …, deserialize = …)` splits the one
                // spelling a wire name is read under.
                syn::Meta::List(list) if list.path.is_ident(key) => {
                    return Err(missing(format!("a list-form `{key}(…)`")));
                }
                _ => {}
            }
        }
    }
    Ok(found)
}

/// A struct variant's own fields never name a variant: only variants are read.
pub(super) fn rust_enum(text: &str, name: &str) -> Result<Names, Unread> {
    let file = rust_file(text)?;
    let items = RustItems::of(&file);
    let what = format!("`enum {name}`");
    let item = one(items.enums.iter().filter(|e| e.ident == name), &what)?;
    if serde_value(&item.attrs, "rename_all")?.as_deref() != Some("snake_case") {
        return Err(missing(format!(
            "{what} without `rename_all = \"snake_case\"`"
        )));
    }
    let names =
        item.variants
            .iter()
            .map(|v| {
                Ok(serde_value(&v.attrs, "rename")?
                    .unwrap_or_else(|| snake_case(&v.ident.to_string())))
            })
            .collect::<Result<Names, Unread>>()?;
    if names.is_empty() {
        return Err(missing(format!("no variant in {what}")));
    }
    Ok(names)
}

/// Its fields, by the name serde reads each under: its `rename`, else its
/// ident, which only a `snake_case` `rename_all` leaves as written; another
/// spelling is never guessed at.
pub(super) fn rust_struct(text: &str, name: &str) -> Result<Names, Unread> {
    let file = rust_file(text)?;
    let items = RustItems::of(&file);
    let what = format!("`struct {name}`");
    let item = one(
        items
            .structs
            .iter()
            .filter(|s| s.ident == name && matches!(s.fields, syn::Fields::Named(_))),
        &what,
    )?;
    if let Some(spelling) = serde_value(&item.attrs, "rename_all")?.filter(|s| s != "snake_case") {
        return Err(missing(format!(
            "{what} with `rename_all = \"{spelling}\"`"
        )));
    }
    let names = item
        .fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(|ident| (f, ident)))
        .map(|(f, ident)| Ok(serde_value(&f.attrs, "rename")?.unwrap_or_else(|| ident.to_string())))
        .collect::<Result<Names, Unread>>()?;
    if names.is_empty() {
        return Err(missing(format!("no field in {what}")));
    }
    Ok(names)
}

/// serde's own `snake_case` (serde_derive's `internals/case.rs`): an `_`
/// before every uppercase letter but the first, then all of it lowercase.
fn snake_case(ident: &str) -> String {
    let mut out = String::new();
    for (i, c) in ident.char_indices() {
        if i > 0 && c.is_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Every `match` arm inside a fn, nested matches included.
#[derive(Default)]
struct Arms<'ast>(Vec<&'ast syn::Arm>);

impl<'ast> Visit<'ast> for Arms<'ast> {
    fn visit_arm(&mut self, i: &'ast syn::Arm) {
        self.0.push(i);
        syn::visit::visit_arm(self, i);
    }
}

/// The string literals a pattern matches: a literal, or an or-pattern of them.
fn pattern_strings(pat: &syn::Pat) -> Vec<String> {
    match pat {
        syn::Pat::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => vec![s.value()],
        syn::Pat::Or(or) => or.cases.iter().flat_map(pattern_strings).collect(),
        syn::Pat::Guard(g) => pattern_strings(&g.pat),
        _ => Vec::new(),
    }
}

/// `src` as its own tokens span it: comments gone, and each gap between two
/// tokens one space, so a pattern reads the code however it is laid out.
fn code_text(src: &str) -> Option<String> {
    use proc_macro2::TokenTree;
    fn spans(ts: proc_macro2::TokenStream, out: &mut Vec<std::ops::Range<usize>>) {
        for t in ts {
            match t {
                TokenTree::Group(g) => {
                    out.push(g.span_open().byte_range());
                    spans(g.stream(), out);
                    out.push(g.span_close().byte_range());
                }
                leaf => out.push(leaf.span().byte_range()),
            }
        }
    }
    let mut ranges = Vec::new();
    spans(src.parse().ok()?, &mut ranges);
    let mut out = String::new();
    let mut end = 0;
    // A doc comment lexes to `#[doc = …]` tokens spanning its own text.
    let mut comment = 0..0;
    for r in ranges {
        let piece = src.get(r.clone())?;
        if comment.contains(&r.start) || piece.starts_with("//") || piece.starts_with("/*") {
            if !comment.contains(&r.start) {
                comment = r.clone();
            }
            continue;
        }
        if r.start > end && !out.is_empty() {
            out.push(' ');
        }
        out.push_str(piece);
        end = r.end;
    }
    Some(out)
}

/// The literal-matching arms of the ONE `fn name` whose body begins with what
/// `arm` matches, read off the arm's own code.
pub(super) fn rust_arms(text: &str, name: &str, arm: &str) -> Result<Names, Unread> {
    let target = Regex::new(&format!(r"\A(?:{arm})"))?;
    let file = rust_file(text)?;
    let items = RustItems::of(&file);
    let (_, body) = one(
        items.fns.iter().filter(|(ident, _)| *ident == name),
        &format!("`fn {name}`"),
    )?;
    let mut arms = Arms::default();
    arms.visit_block(body);
    let found: Names = arms
        .0
        .iter()
        .filter(|a| {
            text.get(a.body.span().byte_range())
                .and_then(code_text)
                .is_some_and(|code| target.is_match(&code))
        })
        .flat_map(|a| pattern_strings(&a.pat))
        .filter(|s| !s.is_empty())
        .collect();
    if found.is_empty() {
        return Err(missing(format!(
            "no arm of `fn {name}` yielding its target"
        )));
    }
    Ok(found)
}

/// The string literal the ONE `item` evaluates to: a const by its name, or a
/// method as `<Type as Trait>::method`, whose body's tail is the literal.
pub(super) fn rust_str(text: &str, item: &str) -> Result<Names, Unread> {
    let spec: syn::ExprPath =
        syn::parse_str(item).map_err(|e| Unread::Ours(format!("`{item}`: {e}")))?;
    let last = |p: &syn::Path| p.segments.last().map(|s| s.ident.to_string());
    let file = rust_file(text)?;
    let items = RustItems::of(&file);
    let what = format!("`{item}`");
    let value: &syn::Expr = match &spec.qself {
        None => {
            let name = last(&spec.path).unwrap_or_default();
            one(
                items
                    .consts
                    .iter()
                    .filter(|(ident, _)| **ident == name)
                    .map(|(_, expr)| *expr),
                &what,
            )?
        }
        Some(qself) => {
            let ty = match qself.ty.as_ref() {
                syn::Type::Path(p) => last(&p.path),
                _ => None,
            };
            let segments: Vec<String> = spec
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            let (method, trait_) = match segments.split_last() {
                Some((m, t)) => (m.clone(), t.last().cloned()),
                None => return Err(Unread::Ours(format!("`{item}` names no method"))),
            };
            let block = one(
                items
                    .impls
                    .iter()
                    .filter(|i| {
                        i.trait_.as_ref().and_then(|(t, _)| last(t)) == trait_
                            && matches!(i.self_ty.as_ref(), syn::Type::Path(p) if last(&p.path) == ty)
                    })
                    .flat_map(|i| i.items.iter())
                    .filter_map(|it| match it {
                        syn::ImplItem::Fn(f) if f.sig.ident == method => Some(&f.block),
                        _ => None,
                    }),
                &what,
            )?;
            match block.stmts.last() {
                Some(syn::Stmt::Expr(tail, None)) => tail,
                _ => return Err(missing(format!("{what} with no tail expression"))),
            }
        }
    };
    match value {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Ok([s.value()].into()),
        _ => Err(missing(format!("{what} not a string literal"))),
    }
}

/// Only each row's header, and only a PascalCase one: the rows name an enum's
/// variants.
pub(super) fn rust_macro_rows(text: &str, name: &str) -> Result<Names, Unread> {
    let file = rust_file(text)?;
    let items = RustItems::of(&file);
    let mac = one(
        items.macros.iter().filter(|m| m.path.is_ident(name)),
        &format!("`{name}!` table"),
    )?;
    let tokens: Vec<proc_macro2::TokenTree> = mac.tokens.clone().into_iter().collect();
    let found: Names = tokens
        .split(|t| matches!(t, proc_macro2::TokenTree::Punct(p) if p.as_char() == ','))
        .filter_map(|row| match without_attrs(row) {
            [
                proc_macro2::TokenTree::Ident(header),
                proc_macro2::TokenTree::Group(g),
                ..,
            ] if g.delimiter() == proc_macro2::Delimiter::Brace => Some(header.to_string()),
            _ => None,
        })
        .filter(|h| h.starts_with(|c: char| c.is_ascii_uppercase()))
        .collect();
    if found.is_empty() {
        return Err(missing(format!("no row in `{name}!`")));
    }
    Ok(found)
}

/// `row` past its leading attributes: a doc comment arrives as `#[doc = …]`.
fn without_attrs(mut row: &[proc_macro2::TokenTree]) -> &[proc_macro2::TokenTree] {
    use proc_macro2::{Delimiter, TokenTree};
    loop {
        match row {
            [TokenTree::Punct(p), TokenTree::Group(g), rest @ ..]
                if p.as_char() == '#' && g.delimiter() == Delimiter::Bracket =>
            {
                row = rest;
            }
            _ => return row,
        }
    }
}

/// The TS declarations a reader may want: top-level, exported, and inside a
/// `declare module`, `declare global` or namespace block.
fn ts_declarations<'b, 'a>(stmts: &'b [Statement<'a>], out: &mut Vec<&'b Declaration<'a>>) {
    for stmt in stmts {
        let decl = match stmt.as_module_declaration() {
            Some(ModuleDeclaration::ExportDeclaration(e)) => Some(&e.declaration),
            _ => stmt.as_declaration(),
        };
        let Some(decl) = decl else { continue };
        out.push(decl);
        match decl {
            Declaration::TSExternalModuleDeclaration(m) => {
                if let Some(block) = &m.body {
                    ts_declarations(&block.body, out);
                }
            }
            Declaration::TSNamespaceDeclaration(ns) => {
                let mut body = &ns.body;
                loop {
                    match body {
                        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
                            body = &inner.body;
                        }
                        TSNamespaceDeclarationBody::TSModuleBlock(block) => {
                            ts_declarations(&block.body, out);
                            break;
                        }
                    }
                }
            }
            Declaration::TSGlobalDeclaration(g) => ts_declarations(&g.body.body, out),
            _ => {}
        }
    }
}

/// `f` over the declarations of a TS document oxc parses without a diagnostic.
fn with_ts<T>(
    text: &str,
    f: impl FnOnce(&[&Declaration<'_>]) -> Result<T, Unread>,
) -> Result<T, Unread> {
    let alloc = Allocator::default();
    let parsed = oxc_parser::Parser::new(&alloc, text, oxc_span::SourceType::ts()).parse();
    // A fatal error files a diagnostic too, so this one check refuses both.
    if let Some(d) = parsed.diagnostics.first() {
        return Err(missing(format!("TypeScript that oxc cannot parse ({d})")));
    }
    let mut decls = Vec::new();
    ts_declarations(&parsed.program.body, &mut decls);
    f(&decls)
}

/// The top-level fields of the ONE interface or type alias `name`, as
/// [`field_facts`] yields them; a nested object's fields and its methods are
/// not fields of it.
pub(super) fn ts_fields(text: &str, name: &str) -> Result<Declared, Unread> {
    with_ts(text, |decls| {
        let decl = one(
            decls.iter().filter(|d| match d {
                Declaration::TSInterfaceDeclaration(i) => i.id.name == name,
                Declaration::TSTypeAliasDeclaration(t) => t.id.name == name,
                _ => false,
            }),
            &format!("interface or type `{name}`"),
        )?;
        let mut why = BTreeSet::new();
        let mut members = Vec::new();
        let what = match decl {
            Declaration::TSInterfaceDeclaration(i) => {
                let what = format!("`interface {name}`");
                if !i.extends.is_empty() {
                    why.insert(format!("{what} extends another type"));
                }
                members.extend(i.body.body.iter());
                what
            }
            Declaration::TSTypeAliasDeclaration(t) => {
                let what = format!("`type {name}`");
                let mut named = false;
                if !object_members(&t.type_annotation, &mut members, &mut named) {
                    return Err(missing(format!("{what}, not an object type")));
                }
                if named {
                    why.insert(format!("{what} intersects a named type"));
                }
                what
            }
            _ => return Err(missing(format!("no interface or type `{name}`"))),
        };
        let mut names = Names::new();
        for member in members {
            if let TSSignature::TSPropertySignature(p) = member {
                if p.computed {
                    why.insert(format!("{what} has a computed key"));
                } else {
                    field_facts(p, &mut names);
                }
            }
        }
        if names.is_empty() {
            return Err(missing(format!("no field in {what}")));
        }
        Ok(Declared {
            names,
            partial: joined(why),
        })
    })
}

/// The members of `ty`'s object-literal operands, through intersections and
/// parentheses, noting an operand that names a type instead; false when `ty`
/// is no object type.
fn object_members<'b, 'a>(
    ty: &'b TSType<'a>,
    members: &mut Vec<&'b TSSignature<'a>>,
    named: &mut bool,
) -> bool {
    match ty {
        TSType::TSTypeLiteral(l) => {
            members.extend(l.members.iter());
            true
        }
        TSType::TSParenthesizedType(p) => object_members(&p.type_annotation, members, named),
        TSType::TSIntersectionType(i) => i.types.iter().all(|t| match t {
            TSType::TSTypeReference(_) => {
                *named = true;
                true
            }
            t => object_members(t, members, named),
        }),
        _ => false,
    }
}

/// A field, `field?` when optional, and `field:"lit"` per string literal its
/// type admits.
fn field_facts(p: &TSPropertySignature<'_>, out: &mut Names) {
    let Some(key) = p.key.static_name() else {
        return;
    };
    out.insert(key.to_string());
    if p.optional {
        out.insert(format!("{key}?"));
    }
    let admits = match p.type_annotation.as_ref().map(|a| &a.type_annotation) {
        Some(TSType::TSUnionType(u)) => u.types.iter().collect(),
        Some(t) => vec![t],
        None => vec![],
    };
    for t in admits {
        if let TSType::TSLiteralType(l) = t
            && let TSLiteral::StringLiteral(lit) = &l.literal
        {
            out.insert(format!("{key}:\"{}\"", lit.value));
        }
    }
}

/// Every member key of the interfaces and classes named `of`, merged across
/// their declarations in the document as TS merges augmentations. A private or
/// protected member is still the runtime's; a `#private` one is no API.
pub(super) fn ts_members(text: &str, of: &[&str]) -> Result<Declared, Unread> {
    with_ts(text, |decls| {
        let mut names = Names::new();
        let mut why = BTreeSet::new();
        let mut any = false;
        let mut key_of = |what: &str, key: &PropertyKey<'_>, computed: bool| {
            if computed {
                why.insert(format!("{what} has a computed key"));
            } else if let Some(k) = key.static_name() {
                names.insert(k.into_owned());
            }
        };
        let mut extends = BTreeSet::new();
        for d in decls {
            match d {
                Declaration::TSInterfaceDeclaration(i) if of.contains(&i.id.name.as_str()) => {
                    any = true;
                    let what = format!("`interface {}`", i.id.name);
                    if !i.extends.is_empty() {
                        extends.insert(format!("{what} extends another type"));
                    }
                    for m in &i.body.body {
                        match m {
                            TSSignature::TSPropertySignature(p) => {
                                key_of(&what, &p.key, p.computed)
                            }
                            TSSignature::TSMethodSignature(m) => key_of(&what, &m.key, m.computed),
                            _ => {}
                        }
                    }
                }
                Declaration::ClassDeclaration(c)
                    if c.id
                        .as_ref()
                        .is_some_and(|id| of.contains(&id.name.as_str())) =>
                {
                    any = true;
                    let what = format!(
                        "`class {}`",
                        c.id.as_ref().map_or("", |id| id.name.as_str())
                    );
                    if c.heritage.is_some() {
                        extends.insert(format!("{what} extends another type"));
                    }
                    for e in &c.body.body {
                        match e {
                            ClassElement::MethodDefinition(m)
                                if m.kind != MethodDefinitionKind::Constructor =>
                            {
                                key_of(&what, &m.key, m.computed);
                            }
                            ClassElement::PropertyDefinition(p) => {
                                key_of(&what, &p.key, p.computed)
                            }
                            ClassElement::AccessorProperty(a) => key_of(&what, &a.key, a.computed),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let listed = || {
            of.iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if !any {
            return Err(missing(format!("no interface or class {}", listed())));
        }
        if names.is_empty() {
            return Err(missing(format!(
                "no member in interface or class {}",
                listed()
            )));
        }
        why.extend(extends);
        Ok(Declared {
            names,
            partial: joined(why),
        })
    })
}

/// Every `return` of the ONE `function name`, in order, as one name: its
/// nested functions' and classes' returns are theirs.
pub(super) fn ts_return(text: &str, name: &str) -> Result<Names, Unread> {
    with_ts(text, |decls| {
        let what = format!("`function {name}`");
        let body = one(
            decls.iter().filter_map(|d| match d {
                Declaration::FunctionDeclaration(f)
                    if f.id.as_ref().is_some_and(|id| id.name == name) =>
                {
                    f.body.as_deref()
                }
                _ => None,
            }),
            &what,
        )?;
        let mut returns = Returns::default();
        returns.visit_function_body(body);
        if returns.spans.is_empty() {
            return Err(missing(format!("no `return` in {what}")));
        }
        let list: Vec<&str> = returns
            .spans
            .iter()
            .map(|s| {
                text.get(s.start as usize..s.end as usize)
                    .unwrap_or_default()
            })
            .collect();
        Ok(Names::from([list.join("; ")]))
    })
}

/// The spans of the returned expressions at the visited body's own depth.
#[derive(Default)]
struct Returns {
    depth: usize,
    spans: Vec<oxc_span::Span>,
}

impl<'a> oxc_ast_visit::Visit<'a> for Returns {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if matches!(
            kind,
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) | AstKind::Class(_)
        ) {
            self.depth += 1;
        }
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        if matches!(
            kind,
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) | AstKind::Class(_)
        ) {
            self.depth -= 1;
        }
    }

    // Not walked: a returned expression holds no `return` of this function.
    fn visit_return_statement(&mut self, it: &ReturnStatement<'a>) {
        if self.depth == 0 {
            self.spans
                .push(it.argument.as_ref().map_or(it.span, |a| a.span()));
        }
    }
}

/// The string elements of the ONE `const name = [ … ]`: the type derived from
/// it and every comparison against it repeat its names but declare none.
pub(super) fn ts_const_array(text: &str, name: &str) -> Result<Names, Unread> {
    with_ts(text, |decls| {
        let array = one(
            const_inits(decls, name).filter_map(|init| match init {
                Expression::ArrayExpression(a) => Some(a),
                _ => None,
            }),
            &format!("`const {name}` array"),
        )?;
        Ok(array
            .elements
            .iter()
            .filter_map(|e| match e {
                ArrayExpressionElement::StringLiteral(s) => Some(s.value.as_str().to_owned()),
                _ => None,
            })
            .collect())
    })
}

/// The literal the ONE `const name` initializes to: a string's value, or a
/// number's, which a reformat of its source text leaves as it was.
pub(super) fn ts_const(text: &str, name: &str) -> Result<Names, Unread> {
    with_ts(text, |decls| {
        let value = one(
            const_inits(decls, name).filter_map(|init| match init {
                Expression::StringLiteral(s) => Some(s.value.as_str().to_owned()),
                Expression::NumericLiteral(n) => Some(n.value.to_string()),
                _ => None,
            }),
            &format!("`const {name}` literal"),
        )?;
        Ok(Names::from([value]))
    })
}

/// What each `const name` declarator initializes to, `as` and parentheses
/// stripped.
fn const_inits<'b, 'a>(
    decls: &'b [&'b Declaration<'a>],
    name: &'b str,
) -> impl Iterator<Item = &'b Expression<'a>> {
    decls
        .iter()
        .filter_map(|d| match d {
            Declaration::VariableDeclaration(v) if v.kind == VariableDeclarationKind::Const => {
                Some(v.declarations.iter())
            }
            _ => None,
        })
        .flatten()
        .filter(move |v| {
            matches!(&v.id, BindingPattern::BindingIdentifier(b) if b.name.as_str() == name)
        })
        .filter_map(|v| Some(v.init.as_ref()?.get_inner_expression()))
}

/// A heading's events: where it starts and ends, its level, and its text.
struct Heading {
    start: usize,
    end: usize,
    level: usize,
    text: String,
}

/// The events of the ONE heading `under`'s section, up to the next heading at
/// its level or above, and that level. Fenced code arrives as text, never as a
/// heading, so a shell sample's `# comment` cannot end the section.
fn md_section<'t>(text: &'t str, under: &str) -> Result<(Vec<Event<'t>>, usize), Unread> {
    let events: Vec<Event<'t>> = Parser::new_ext(text, Options::ENABLE_TABLES).collect();
    let headings = md_headings_of(&events);
    let parent = one(
        headings.iter().filter(|h| h.text.trim() == under),
        &format!("`{under}` heading"),
    )?;
    let end = headings
        .iter()
        .find(|h| h.start > parent.end && h.level <= parent.level)
        .map_or(events.len(), |h| h.start);
    Ok((events[parent.end + 1..end].to_vec(), parent.level))
}

fn md_headings_of(events: &[Event<'_>]) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut open: Option<(usize, usize, String)> = None;
    for (i, event) in events.iter().enumerate() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                open = Some((i, *level as usize, String::new()));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, _, text)) = open.as_mut() {
                    text.push_str(t);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((start, level, text)) = open.take() {
                    out.push(Heading {
                        start,
                        end: i,
                        level,
                        text,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// The backticked first-column names of the first table in the section whose
/// header row begins with `header`: a page's later tables repeat its names.
pub(super) fn md_table(text: &str, header: &str, under: &str) -> Result<Names, Unread> {
    let (section, _) = md_section(text, under)?;
    // Each table as rows of cells, each cell its events.
    let mut tables: Vec<Vec<Vec<Vec<Event<'_>>>>> = Vec::new();
    let mut cell: Option<Vec<Event<'_>>> = None;
    for event in section {
        match event {
            Event::Start(Tag::Table(_)) => tables.push(Vec::new()),
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                if let Some(t) = tables.last_mut() {
                    t.push(Vec::new());
                }
            }
            Event::Start(Tag::TableCell) => cell = Some(Vec::new()),
            Event::End(TagEnd::TableCell) => {
                if let (Some(c), Some(row)) =
                    (cell.take(), tables.last_mut().and_then(|t| t.last_mut()))
                {
                    row.push(c);
                }
            }
            e => {
                if let Some(c) = cell.as_mut() {
                    c.push(e);
                }
            }
        }
    }
    let cell_text = |c: &[Event<'_>]| -> String {
        c.iter()
            .map(|e| match e {
                Event::Text(t) | Event::Code(t) => t.as_ref(),
                _ => "",
            })
            .collect()
    };
    let table = tables
        .iter()
        .find(|t| {
            t.first()
                .and_then(|head| head.first())
                .is_some_and(|c| cell_text(c).trim() == header)
        })
        .ok_or_else(|| missing(format!("no `{header}` table")))?;
    let found: Names = table
        .iter()
        .skip(1)
        .filter_map(|row| match row.first().map(Vec::as_slice) {
            Some([Event::Code(name)]) => Some(name.to_string()),
            _ => None,
        })
        .collect();
    if found.is_empty() {
        return Err(missing(format!("no name in the `{header}` table")));
    }
    Ok(found)
}

/// The one-word titles of the direct subsections of the ONE heading `under`; a
/// page's other sections carry one-word headings of their own.
pub(super) fn md_headings(text: &str, under: &str) -> Result<Names, Unread> {
    let (section, level) = md_section(text, under)?;
    let word = Regex::new(r"\A[A-Za-z_][A-Za-z0-9_]*\z")?;
    let found: Names = md_headings_of(&section)
        .into_iter()
        .filter(|h| h.level == level + 1)
        .map(|h| h.text.trim().to_owned())
        .filter(|t| word.is_match(t))
        .collect();
    if found.is_empty() {
        return Err(missing(format!("no one-word heading under `{under}`")));
    }
    Ok(found)
}

/// Comments go first: a comment quoting `{"action": "continue"}` once ended
/// the set at its stray `}`. An empty set is an answer, not a miss. Hand-rolled
/// on purpose: a Python grammar crate for the three set literals hermes' watches
/// read (`install/hermes.rs`) outweighs this scanner.
pub(super) fn py_set(text: &str, decl: &str) -> Result<Names, Unread> {
    let src = strip_py_comments(text);
    let opener = Regex::new(&format!(r"(?:{decl})\s*\{{"))?;
    let what = "set its pattern declares";
    let start = one(opener.find_iter(&src), what)?.end();
    let mut depth = 1usize;
    let close = src[start..]
        .char_indices()
        .find(|&(_, c)| {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            depth == 0
        })
        .map(|(i, _)| start + i)
        .ok_or_else(|| missing(format!("no end to {what}")))?;
    let member = Regex::new(r#""([^"\\\n]+)""#)?;
    Ok(member
        .captures_iter(&src[start..close])
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned()))
        .collect())
}

/// Python source without `#` comments, string literals kept.
fn strip_py_comments(src: &str) -> String {
    src.lines()
        .map(|line| {
            let mut quote = None;
            for (i, c) in line.char_indices() {
                match (quote, c) {
                    (None, '#') => return &line[..i],
                    (None, '"' | '\'') => quote = Some(c),
                    (Some(q), _) if c == q => quote = None,
                    _ => {}
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Scoped to the union ON PURPOSE: every definition's `type` const also yields
/// nested-content tags (`audio`, `text`) that no envelope carries.
pub(super) fn json_union(text: &str, def: &str, prop: Option<&str>) -> Result<Names, Unread> {
    let root: Value = serde_json::from_str(text).map_err(|_| missing("no JSON"))?;
    union_members(&root, def, prop)
        .filter(|found| !found.is_empty())
        .ok_or_else(|| missing(format!("no `{def}` union")))
}

fn union_members(root: &Value, def: &str, prop: Option<&str>) -> Option<Names> {
    let defs = root.get("$defs").or_else(|| root.get("definitions"))?;
    let union = defs.get(def)?;
    let members = union
        .get("oneOf")
        .or_else(|| union.get("anyOf"))?
        .as_array()?;
    Some(
        members
            .iter()
            .filter_map(|m| {
                let node = match m.get("$ref").and_then(Value::as_str) {
                    Some(r) => defs.get(r.rsplit('/').next()?)?,
                    None => m,
                };
                let node = match prop {
                    Some(p) => node.get("properties")?.get(p)?,
                    None => node,
                };
                schema_const(node)
            })
            .collect(),
    )
}

/// A schema node's pinned string: `const`, or a one-element `enum`.
fn schema_const(node: &Value) -> Option<String> {
    let c = match node.get("const") {
        Some(c) => c,
        None => match node.get("enum")?.as_array()?.as_slice() {
            [only] => only,
            _ => return None,
        },
    };
    c.as_str().map(str::to_owned)
}

pub(super) fn json_keys(text: &str) -> Result<Names, Unread> {
    fn walk(v: &Value, out: &mut Names) {
        match v {
            Value::Object(o) => {
                if let Some(Value::Object(props)) = o.get("properties") {
                    out.extend(props.keys().cloned());
                }
                o.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let root: Value = serde_json::from_str(text).map_err(|_| missing("no JSON"))?;
    let mut out = Names::new();
    walk(&root, &mut out);
    if out.is_empty() {
        return Err(missing("no `properties`"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names, or what the reader found instead; a pattern of ours that
    /// fails to compile fails the test.
    fn found(read: Result<impl Into<Declared>, Unread>) -> Result<Vec<String>, String> {
        match read {
            Ok(read) => Ok(read.into().names.into_iter().collect()),
            Err(Unread::Missing(what)) => Err(what),
            Err(Unread::Ours(e)) => panic!("a test pattern does not compile: {e}"),
        }
    }

    fn names(xs: &[&str]) -> Result<Vec<String>, String> {
        Ok(xs.iter().map(|s| (*s).to_owned()).collect())
    }

    /// Why the read is partial; a read that failed fails the test.
    fn partial(read: Result<Declared, Unread>) -> Option<String> {
        match read {
            Ok(read) => read.partial,
            Err(Unread::Missing(what)) => panic!("the read found {what}"),
            Err(Unread::Ours(e)) => panic!("a test pattern does not compile: {e}"),
        }
    }

    #[test]
    fn a_json_union_resolves_refs_consts_and_single_enums() {
        let schema = r##"{"$defs": {
            "U": {"oneOf": [
                {"$ref": "#/$defs/A"},
                {"properties": {"tag": {"const": "inline"}}},
                {"properties": {"tag": {"enum": ["single"]}}},
                {"properties": {"tag": {"enum": ["one", "two"]}}}
            ]},
            "A": {"properties": {"tag": {"const": "by_ref"}}},
            "S": {"oneOf": [{"const": "done"}, {"const": "failed"}]},
            "Nested": {"properties": {"tag": {"const": "never_in_the_union"}}}
        }}"##;
        assert_eq!(
            found(json_union(schema, "U", Some("tag"))),
            names(&["by_ref", "inline", "single"])
        );
        assert_eq!(
            found(json_union(schema, "S", None)),
            names(&["done", "failed"])
        );
        assert_eq!(
            found(json_union(schema, "Gone", None)),
            Err("no `Gone` union".to_owned())
        );
        assert_eq!(
            found(json_union("not json", "U", Some("tag"))),
            Err("no JSON".to_owned())
        );
        // The draft-07 spelling: `definitions` and `anyOf`.
        let draft7 = r##"{"definitions": {
            "U": {"anyOf": [{"$ref": "#/definitions/A"}]},
            "A": {"properties": {"tag": {"const": "by_ref"}}}
        }}"##;
        assert_eq!(
            found(json_union(draft7, "U", Some("tag"))),
            names(&["by_ref"])
        );
    }

    #[test]
    fn a_rust_str_is_the_literal_its_one_item_evaluates_to() {
        let src = r#"
            pub(crate) const METHOD: &str = "_x.ai/session/update";
            const OTHER: u8 = 1;
            impl ToolSpec for AgentTool {
                fn name(&self) -> &'static str {
                    // "old"
                    "agent"
                }
            }
            impl ToolSpec for ShellTool {
                fn name(&self) -> &'static str { "shell" }
            }
            impl Display for AgentTool {
                fn name(&self) -> &'static str { "display" }
            }
            impl AgentTool {
                const KIND: &str = "subagent";
            }"#;
        assert_eq!(
            found(rust_str(src, "METHOD")),
            names(&["_x.ai/session/update"])
        );
        assert_eq!(
            found(rust_str(src, "<AgentTool as ToolSpec>::name")),
            names(&["agent"])
        );
        assert_eq!(found(rust_str(src, "KIND")), names(&["subagent"]));
        assert_eq!(
            found(rust_str(src, "OTHER")),
            Err("`OTHER` not a string literal".to_owned())
        );
        assert_eq!(
            found(rust_str(src, "<AgentTool as ToolSpec>::description")),
            Err("no `<AgentTool as ToolSpec>::description`".to_owned())
        );
        assert!(matches!(rust_str(src, "<<>"), Err(Unread::Ours(_))));
        let twice = format!("{src}\n{src}");
        for item in ["METHOD", "<AgentTool as ToolSpec>::name"] {
            assert_eq!(
                found(rust_str(&twice, item)),
                Err(format!("more than one `{item}`"))
            );
        }
    }

    #[test]
    fn code_text_is_the_code_without_its_comments() {
        assert_eq!(
            code_text("Ok( /* c */ a::B ) // d").as_deref(),
            Some("Ok( a::B )")
        );
        // A doc comment lexes to `#[doc = …]` tokens that all span its text.
        assert_eq!(
            code_text("/// doc\n#[allow(x)] Ok(b)").as_deref(),
            Some("#[allow(x)] Ok(b)")
        );
    }

    #[test]
    fn a_document_its_parser_rejects_is_never_read() {
        let rejected = |r: Result<Declared, Unread>, lang: &str| {
            let what = found(r);
            assert!(
                what.as_ref().is_err_and(|w| w.starts_with(lang)),
                "{what:?}"
            );
        };
        rejected(
            rust_enum("enum E {", "E").map(Into::into),
            "Rust that syn cannot parse (",
        );
        rejected(
            ts_fields("export interface E {", "E"),
            "TypeScript that oxc cannot parse (",
        );
        // Recoverable: oxc keeps the whole tree beside the diagnostic.
        rejected(
            ts_fields("export interface E { a: string }\nconst x;\n", "E"),
            "TypeScript that oxc cannot parse (",
        );
    }

    #[test]
    fn rust_arms_read_only_the_arms_yielding_the_target() {
        let arm = r"Ok\(\s*Action::(?:Start|Wait)\s*\)";
        let src = r#"
            fn parse(input: &str) -> Result<Action> {
                match input {
                    // "commented" => Ok(Action::Start),
                    "start" | "spawn" => Ok(Action::Start),
                    "wait" => Ok( Action::Wait ),
                    "noted" => Ok(/* the original */ Action::Start),
                    "guarded" if ready => Ok(Action::Start),
                    "outer" => match tail { "nested" => Ok(Action::Start), _ => Err(()) },
                    "wrapped" => wrap(Ok(Action::Start)),
                    "list" => Ok(Action::List),
                    _ => { Err(()) }
                }
            }
            fn other(b: &str) -> Result<Action> {
                match b { "x" => Ok(Action::Start), _ => Err(()) }
            }"#;
        let read = |t: &str| found(rust_arms(t, "parse", arm));
        // A comment inside an arm is no part of its code; a guarded arm still
        // decodes its literal; a nested match's arm is read; an arm only
        // wrapping the target, or opening a match, yields it at no start.
        assert_eq!(
            read(src),
            names(&["guarded", "nested", "noted", "spawn", "start", "wait"])
        );
        let default = "trait T {\n    fn parse(i: &str) -> Result<Action> {\n        match i { \"t\" => Ok(Action::Start), _ => Err(()) }\n    }\n}";
        assert_eq!(read(default), names(&["t"]));
        assert_eq!(read("fn parsed() {}"), Err("no `fn parse`".to_owned()));
        assert_eq!(
            read("fn parse() { match a { \"list\" => Ok(Action::List) } }"),
            Err("no arm of `fn parse` yielding its target".to_owned())
        );
    }

    #[test]
    fn a_rust_enum_yields_its_serde_wire_names() {
        let src = r#"
            const X: u8 = 1;
            /// Docs, and a `#[serde(rename_all = "camelCase")]` in a comment.
            #[derive(Serialize)]
            #[serde(tag = "type", rename_all = "snake_case")]
            pub(super) enum Item<'a> {
                /// Fires like `Foo { bar }` does, or `Baz }`.
                EventMsg { payload: Cow<'a, Payload> },
                #[serde(rename = "compaction_summary", alias = "compacted")]
                Compaction(Summary),
                HTTPCall,
                Struct {
                    #[serde(rename = "agentAddress")]
                    agent_address: String,
                    nested: Inner<Deep>,
                },
                // Gone,
            }
            enum ItemKind { NotThisOne }"#;
        assert_eq!(
            found(rust_enum(src, "Item")),
            names(&["compaction_summary", "event_msg", "h_t_t_p_call", "struct"])
        );
        assert_eq!(
            found(rust_enum(src, "ItemKind")).map(|_| ()).unwrap_err(),
            "`enum ItemKind` without `rename_all = \"snake_case\"`"
        );
        assert_eq!(
            found(rust_enum(src, "Missing")),
            Err("no `enum Missing`".to_owned())
        );
        // A `#[cfg(test)]` twin is ambiguity, never the first one found.
        assert_eq!(
            found(rust_enum(&format!("{src}\n{src}"), "Item")),
            Err("more than one `enum Item`".to_owned())
        );
        // An attribute's own words never name the variant it sits on.
        let attributed = "#[serde(rename_all = \"snake_case\")]\nenum A {\n    #[doc = \"Other\"]\n    Plain,\n}";
        assert_eq!(found(rust_enum(attributed, "A")), names(&["plain"]));
        // Another spelling is never guessed at: its names would all be wrong.
        let camel = "#[serde(rename_all = \"camelCase\")]\nenum E { OneTwo }";
        assert!(found(rust_enum(camel, "E")).is_err());
    }

    #[test]
    fn a_rust_struct_yields_its_fields_serde_wire_names() {
        let src = r#"
            #[derive(Deserialize)]
            pub struct HookEventsToml {
                #[serde(rename = "PreToolUse", default)]
                pub pre_tool_use: Vec<Group>,
                /// `{ commented: x }`
                #[serde(default)]
                pub(crate) session_start: Vec<Group>,
                #[serde(rename = "Stop", alias = "Halt")]
                pub stop: Vec<Group>,
                #[serde(default = "crate::defaults::events")]
                pub events: Vec<Group>,
            }
            struct HookEventsTomlExtra { not_this: u8 }"#;
        assert_eq!(
            found(rust_struct(src, "HookEventsToml")),
            names(&["PreToolUse", "Stop", "events", "session_start"])
        );
        // Another spelling would re-spell every ident; it is never guessed at.
        let pascal = "#[serde(rename_all = \"PascalCase\")]\nstruct S { one_two: u8 }";
        assert_eq!(
            found(rust_struct(pascal, "S")),
            Err("`struct S` with `rename_all = \"PascalCase\"`".to_owned())
        );
        let snake = "#[serde(rename_all = \"snake_case\")]\nstruct S { one_two: u8 }";
        assert_eq!(found(rust_struct(snake, "S")), names(&["one_two"]));
        assert_eq!(
            found(rust_struct("struct S(u8);", "S")),
            Err("no `struct S`".to_owned())
        );
    }

    #[test]
    fn a_ts_const_array_is_its_declarations_strings_only() {
        let src = "// const NAMES = [\"commented\"];\nconst NAMES = [\n  \"agent_end\",\n  'session_start', // was \"gone\"\n] as const;\nexport type Name = (typeof NAMES)[number];\nconst isOther = (n: Name) => n === \"other\";\n";
        assert_eq!(
            found(ts_const_array(src, "NAMES")),
            names(&["agent_end", "session_start"])
        );
        assert_eq!(
            found(ts_const_array(src, "NAME")),
            Err("no `const NAME` array".to_owned())
        );
        assert!(found(ts_const_array(&format!("{src}{src}"), "NAMES")).is_err());
        assert_eq!(
            found(ts_const_array("let NAMES = [\"x\"];", "NAMES")),
            Err("no `const NAMES` array".to_owned())
        );
        let shapes = "const NAMES = ([\"own\", flag ? \"c\" : \"d\", [\"inner\"], ...[\"spread\"]]) as const;";
        assert_eq!(found(ts_const_array(shapes, "NAMES")), names(&["own"]));
    }

    /// A number reads as its value, so a reformat of the literal is no change.
    #[test]
    fn a_ts_const_is_its_one_literal() {
        let src = "// export const PORT = 1;\nexport const PORT = 18_789;\nconst HEX: number = 0x4965;\nconst NAME = 'agent' as const;\nconst CALL = port();\n";
        assert_eq!(found(ts_const(src, "PORT")), names(&["18789"]));
        assert_eq!(found(ts_const(src, "HEX")), names(&["18789"]));
        assert_eq!(found(ts_const(src, "NAME")), names(&["agent"]));
        assert_eq!(
            found(ts_const(src, "CALL")),
            Err("no `const CALL` literal".to_owned())
        );
        assert_eq!(
            found(ts_const(src, "GONE")),
            Err("no `const GONE` literal".to_owned())
        );
        assert_eq!(
            found(ts_const("let PORT = 1;", "PORT")),
            Err("no `const PORT` literal".to_owned())
        );
        assert!(found(ts_const(&format!("{src}{src}"), "PORT")).is_err());
    }

    #[test]
    fn ts_fields_read_the_one_interface() {
        let src = "export interface Entry extends Base {\n  readonly type: \"x\";\n  title?: string;\n  \"quoted-key\": number;\n  nested: {\n    inner: string;\n  };\n  /** hidden: in a comment */\n  later(): void;\n}\nexport interface Other {\n  title: string;\n  only_other: string;\n}\n";
        // A nested object's fields, a comment and a method are no fields of it.
        assert_eq!(
            found(ts_fields(src, "Entry")),
            names(&[
                "nested",
                "quoted-key",
                "title",
                "title?",
                "type",
                "type:\"x\""
            ])
        );
        assert_eq!(
            partial(ts_fields(src, "Entry")).as_deref(),
            Some("`interface Entry` extends another type")
        );
        assert_eq!(partial(ts_fields(src, "Other")), None);
        assert_eq!(
            found(ts_fields(src, "Gone")),
            Err("no interface or type `Gone`".to_owned())
        );
        let twice = format!("{src}export interface Other {{\n  b: string;\n}}\n");
        assert_eq!(
            found(ts_fields(&twice, "Other")),
            Err("more than one interface or type `Other`".to_owned())
        );
    }

    /// The facts beside a field are names a watch can wait on: `approved?`,
    /// `outcome:"error"`.
    #[test]
    fn ts_fields_yield_each_fields_optionality_and_string_literals() {
        let src = "export interface E {\n  a: string;\n  b?: number;\n  kind: \"x\" | \"y\" | 3;\n  only?: \"z\";\n  [KEY]: number;\n  run(): void;\n}\n";
        assert_eq!(
            found(ts_fields(src, "E")),
            names(&[
                "a",
                "b",
                "b?",
                "kind",
                "kind:\"x\"",
                "kind:\"y\"",
                "only",
                "only:\"z\"",
                "only?"
            ])
        );
        assert_eq!(
            partial(ts_fields(src, "E")).as_deref(),
            Some("`interface E` has a computed key")
        );
    }

    /// A type alias is read through its object-literal operands, however
    /// nested or parenthesised; a named operand's fields are out of sight.
    #[test]
    fn ts_fields_read_a_type_alias_through_its_intersections() {
        let src = "type Plain = { runId?: string; success: boolean };\nexport type Ended = Base & ({ outcome: \"completed\" | \"error\" } & { failureKind?: \"aborted\" });\n";
        assert_eq!(
            found(ts_fields(src, "Plain")),
            names(&["runId", "runId?", "success"])
        );
        assert_eq!(partial(ts_fields(src, "Plain")), None);
        assert_eq!(
            found(ts_fields(src, "Ended")),
            names(&[
                "failureKind",
                "failureKind:\"aborted\"",
                "failureKind?",
                "outcome",
                "outcome:\"completed\"",
                "outcome:\"error\"",
            ])
        );
        assert_eq!(
            partial(ts_fields(src, "Ended")).as_deref(),
            Some("`type Ended` intersects a named type")
        );
        for alias in ["A | B", "Base", "{ [K in Keys]: string }", "Base & (A | B)"] {
            assert_eq!(
                found(ts_fields(&format!("type X = {alias};"), "X")),
                Err("`type X`, not an object type".to_owned()),
                "{alias}"
            );
        }
    }

    /// Every member key of the interfaces and classes named, merged across
    /// their declarations in the document, as TS merges augmentations.
    #[test]
    fn ts_members_read_every_key_of_the_interfaces_and_classes_named() {
        let src = "declare module 'x' {\n  interface Events {\n    'agent/created'(this: S, p: {}): void\n    'agent/status': { a: number }\n  }\n  interface Context { sessions: S }\n}\ninterface SessionEventMap {\n  'turn/start': { turn: number }\n}\ninterface Events {\n  'session/event'(s: S): void\n  'session/event'(s: S, e: E): void\n}\nexport class SessionManager extends Base {\n  private secret = 1;\n  constructor() {}\n  getSessionFile(): string { return ''; }\n  get getter(): number { return 1; }\n  static make() {}\n  #hidden() {}\n  [COMPUTED]() {}\n}\n";
        let events = ["Events", "SessionEventMap"];
        assert_eq!(
            found(ts_members(src, &events)),
            names(&[
                "agent/created",
                "agent/status",
                "session/event",
                "turn/start"
            ])
        );
        assert_eq!(partial(ts_members(src, &events)), None);
        // A private or protected member is still the runtime's.
        assert_eq!(
            found(ts_members(src, &["SessionManager"])),
            names(&["getSessionFile", "getter", "make", "secret"])
        );
        assert_eq!(
            partial(ts_members(src, &["SessionManager"])).as_deref(),
            Some(
                "`class SessionManager` extends another type; `class SessionManager` has a computed key"
            )
        );
        assert_eq!(
            found(ts_members(src, &["Gone", "Lost"])),
            Err("no interface or class `Gone`, `Lost`".to_owned())
        );
        let extended = "interface Events extends Base {\n  'a': 1\n}\n";
        assert_eq!(
            partial(ts_members(extended, &events)).as_deref(),
            Some("`interface Events` extends another type")
        );
        let computed = "interface Events {\n  [K]: 1;\n  'a': 1\n}\ninterface SessionEventMap {\n  [K](): void;\n  'b': 1\n}\nclass C {\n  accessor x = 1;\n}\n";
        for of in ["Events", "SessionEventMap"] {
            assert_eq!(
                partial(ts_members(computed, &[of])),
                Some(format!("`interface {of}` has a computed key"))
            );
        }
        assert_eq!(found(ts_members(computed, &["C"])), names(&["x"]));
    }

    /// Every `return` of the ONE function, in order, its nested functions'
    /// aside: an early-return branch is the redirect a watch waits on.
    #[test]
    fn ts_return_is_every_return_of_the_one_function() {
        let src = "export function getAgentDir(): string {\n  return dirs.agentDir;\n}\nfunction redirected(): string {\n  if (process.env.X) return process.env.X;\n  const f = () => { return 'nested'; };\n  function g() { return 'decl'; }\n  const h = function () { return 'expr'; };\n  class C { m() { return 'method'; } }\n  return dirs.agentDir;\n}\nfunction none() {}\n";
        assert_eq!(
            found(ts_return(src, "getAgentDir")),
            names(&["dirs.agentDir"])
        );
        assert_eq!(
            found(ts_return(src, "redirected")),
            names(&["process.env.X; dirs.agentDir"])
        );
        assert_eq!(
            found(ts_return(src, "none")),
            Err("no `return` in `function none`".to_owned())
        );
        let arrow = "export const getAgentDir = (): string => dirs.agentDir;\n";
        assert_eq!(
            found(ts_return(arrow, "getAgentDir")),
            Err("no `function getAgentDir`".to_owned())
        );
    }

    #[test]
    fn ts_fields_read_an_interface_inside_a_module_block() {
        let src = "declare module '@x/core' {\n  interface Context {\n    session: Session\n  }\n  interface Events {\n    'agent/created'(payload: {}): void\n    status: string\n  }\n}\n";
        assert_eq!(found(ts_fields(src, "Events")), names(&["status"]));
        assert_eq!(partial(ts_fields(src, "Events")), None);
    }

    #[test]
    fn ts_fields_read_an_interface_inside_a_namespace_or_global_block() {
        let src = "export {};\nnamespace A.B {\n  export interface X { f: string }\n}\ndeclare global {\n  interface Y { g: string }\n}\n";
        assert_eq!(found(ts_fields(src, "X")), names(&["f"]));
        assert_eq!(found(ts_fields(src, "Y")), names(&["g"]));
    }

    #[test]
    fn an_unreadable_serde_attribute_is_never_read() {
        let src = "#[serde(rename_all = )]\nenum E { A }";
        assert!(
            found(rust_enum(src, "E"))
                .is_err_and(|what| what.starts_with("an unreadable `#[serde]` attribute"))
        );
        let listed = |r: Result<Names, Unread>, key: &str| {
            assert_eq!(found(r), Err(format!("a list-form `{key}(…)`")));
        };
        let all = "#[serde(rename_all(deserialize = \"PascalCase\"))]\n";
        listed(
            rust_struct(&format!("{all}struct S {{ a: u8 }}"), "S"),
            "rename_all",
        );
        listed(
            rust_enum(&format!("{all}enum E {{ A }}"), "E"),
            "rename_all",
        );
        let one = "#[serde(rename(serialize = \"a\", deserialize = \"b\"))]";
        listed(
            rust_struct(&format!("struct S {{ {one} x: u8 }}"), "S"),
            "rename",
        );
        listed(
            rust_enum(
                &format!("#[serde(rename_all = \"snake_case\")]\nenum E {{ {one} X }}"),
                "E",
            ),
            "rename",
        );
    }

    #[test]
    fn matches_read_every_first_group() {
        let go = "const (\n\tSessionStart Event = \"SessionStart\"\n\tStop Event = \"Stop\"\n)\nvar x = \"Nope\"";
        let p = r#"\w+\s+Event\s*=\s*"(\w+)""#;
        assert_eq!(found(matches(go, p)), names(&["SessionStart", "Stop"]));
        assert_eq!(
            found(matches("const ()", p)),
            Err("no match of its pattern".to_owned())
        );
    }

    #[test]
    fn an_md_table_is_read_by_its_header_and_first_column() {
        let page = "## Intro\n| Event | x |\n|---|---|\n| `Before` | x |\n## Lifecycle\n| Event | When it fires |\n|---|---|\n| `PreToolUse` | before `Bash` |\n| `Stop` | done |\n\n| Field | Event |\n|---|---|\n| `x` | `Other` |\n\n| Event | Payload |\n|---|---|\n| `Later` | x |\n";
        assert_eq!(
            found(md_table(page, "Event", "Lifecycle")),
            names(&["PreToolUse", "Stop"])
        );
        assert_eq!(
            found(md_table(page, "Hook", "Lifecycle")),
            Err("no `Hook` table".to_owned())
        );
        let coded = "## Lifecycle\n| `Event` | x |\n|---|---|\n| `Stop` | done |\n";
        assert_eq!(
            found(md_table(coded, "Event", "Lifecycle")),
            names(&["Stop"])
        );
        let setext = "Lifecycle\n---------\nEvent | x\n---|---\n`Stop` | done\n";
        assert_eq!(
            found(md_table(setext, "Event", "Lifecycle")),
            names(&["Stop"])
        );
        assert_eq!(
            found(md_table(page, "Event", "Gone")),
            Err("no `Gone` heading".to_owned())
        );
        assert_eq!(
            found(md_table(&format!("{page}{page}"), "Event", "Lifecycle")),
            Err("more than one `Lifecycle` heading".to_owned())
        );
    }

    #[test]
    fn md_headings_are_the_one_word_ones_in_their_section() {
        let page = "# Hooks\n## Quickstart\n```sh\n# Read input\n```\n## Reference\n### Hook events\n```sh\n# stdin\n```\n#### preToolUse\n#### `postToolUse`\n#### before / after\n##### deeper\nprose stop\n### Other\n#### elsewhere\n## Troubleshooting\n";
        assert_eq!(
            found(md_headings(page, "Hook events")),
            names(&["postToolUse", "preToolUse"])
        );
        for end in ["> ### Quoted", "Other\n-----"] {
            let ended = format!("### Hook events\n#### a\n{end}\n#### b\n");
            assert_eq!(
                found(md_headings(&ended, "Hook events")),
                names(&["a"]),
                "{end}"
            );
        }
        assert_eq!(
            found(md_headings(page, "Gone")),
            Err("no `Gone` heading".to_owned())
        );
        assert_eq!(
            found(md_headings(&format!("{page}{page}"), "Hook events")),
            Err("more than one `Hook events` heading".to_owned())
        );
    }

    #[test]
    fn a_py_set_is_the_one_declaration_comments_first() {
        let src = "VALID_HOOKS: set[str] = {\n    \"on_start\",  # was {\"x\"} once\n    \"a#b\",  # real\n    \"on_end\",\n}\n";
        let decl = r"VALID_HOOKS\s*:\s*set\[str\]\s*=";
        assert_eq!(
            found(py_set(src, decl)),
            names(&["a#b", "on_end", "on_start"])
        );
        let frozen = "_BLOCKING_EVENTS = frozenset({\"pre_tool_call\"})";
        assert_eq!(
            found(py_set(frozen, r"_BLOCKING_EVENTS\s*=\s*frozenset\(")),
            names(&["pre_tool_call"])
        );
        for comment in [
            "# returns {\"action\": \"continue\"",
            "# anything else } ends it",
        ] {
            let poisoned = format!(
                "VALID_HOOKS: set[str] = {{\n    \"on_start\",\n    {comment}\n    \"on_end\",\n}}\n"
            );
            assert_eq!(
                found(py_set(&poisoned, decl)),
                names(&["on_end", "on_start"])
            );
        }
        assert_eq!(found(py_set("X = {\n}", r"X\s*=")), names(&[]));
        assert_eq!(
            found(py_set("X = {\"a\", {\"b\"}, \"c\"}\nY = {\"d\"}", r"X\s*=")),
            names(&["a", "b", "c"])
        );
        assert_eq!(
            found(py_set("X = {\"a\",", r"X\s*=")),
            Err("no end to set its pattern declares".to_owned())
        );
        assert!(found(py_set(&format!("{src}{src}"), decl)).is_err());
    }

    #[test]
    fn macro_rows_are_the_row_headers_only() {
        let src = "macro_rules! hook_events {\n    ($($v:ident { $($r:tt)* }),*) => { pub enum HookEventName { $($v,)* } };\n}\nhook_events! {\n    SessionStart { alias: \"start\", traits: [Blocking] },\n    // Gone { },\n    /// Legacy alias of `Stop`.\n    #[cfg(unix)]\n    StopEnd { alias: \"stop_end\" },\n    Stop { alias: \"stop\" },\n    internal { alias: \"x\" },\n}\n";
        // A row's doc comment and attributes precede its header, not replace it.
        assert_eq!(
            found(rust_macro_rows(src, "hook_events")),
            names(&["SessionStart", "Stop", "StopEnd"])
        );
        assert_eq!(
            found(rust_macro_rows("fn x() {}", "hook_events")),
            Err("no `hook_events!` table".to_owned())
        );
        assert_eq!(
            found(rust_macro_rows(&format!("{src}{src}"), "hook_events")),
            Err("more than one `hook_events!` table".to_owned())
        );
    }

    #[test]
    fn json_keys_walk_every_depth() {
        let schema = r#"{"properties": {"a": {}, "data": {"properties": {"b": {}}}},
                         "definitions": {"X": {"anyOf": [{"properties": {"c": {}}}]}}}"#;
        assert_eq!(found(json_keys(schema)), names(&["a", "b", "c", "data"]));
        assert_eq!(
            found(json_keys(r#"{"type": "object"}"#)),
            Err("no `properties`".to_owned())
        );
        assert_eq!(found(json_keys("not json")), Err("no JSON".to_owned()));
    }
}
