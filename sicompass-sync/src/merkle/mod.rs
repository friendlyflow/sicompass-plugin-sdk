//! A store's Merkle tree: the hash of every object, so two copies of a store
//! can tell which objects differ without comparing their contents.
//!
//! # The hash definition is a wire format
//!
//! Both plugins, the server and every peer compute the same bytes, so the
//! definition below is a contract, not an implementation detail. Changing it
//! invalidates every stored `.listmeta` and every comparison already made.
//!
//! ```text
//! hash(leaf)   = sha256( b"s\0" || text )
//! hash(branch) = sha256( b"o\0" || text || b"\0" || d0 || d1 || ... )
//! root         = sha256( b"r\0" || d0 || d1 || ... )
//! ```
//!
//! where `d0..dn` are the children's **raw 32-byte digests** in list order, not
//! their hex forms. The `s` / `o` / `r` prefixes are domain separation: without
//! them a leaf whose text is `x` and a childless branch whose text is `x` would
//! hash alike, and a tree could be restructured without changing its root.
//!
//! Deliberately **not** hashed: ids (local bookkeeping, so two machines that
//! built the same tree agree on its root), and every other field a plugin keeps
//! in `.listmeta` (notes' `visibility`, the board's `archive`): a change of
//! audience or of filing, not of content. Those travel as [`Extras`], and the
//! flat snapshot hash ([`crate::snapshot::Snapshot::hash`]) is what notices
//! when they change.
//!
//! # The store layout
//!
//! Every plugin that syncs keeps the same layout (see
//! [`crate::snapshot::is_safe_store_path`]):
//!
//! ```text
//! .listmeta   {"sha256":"<root>","children":[{"n":1,"id":7,"sha256":"<d0>"}]}
//! 0001        the first object's text, verbatim
//! 0001.d/     its children, when it is a branch
//!   .listmeta {"sha256":"<hash of 0001>","children":[...]}
//! ```
//!
//! [`parse`] reads that into a [`StoreTree`], [`to_files`] writes one back with
//! every hash recomputed, [`verify`] checks the hashes a store carries, and
//! [`diff`] names the objects two trees disagree on. [`merge`] is the
//! three-way merge the sync runs on top.

pub mod merge;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

pub use merge::{Merged, Side, merge3};

/// An object's identity in its store: minted by the plugin, never reused,
/// never hashed. `0` means "not known yet" and never survives [`parse`].
pub type Id = u64;

/// The `.listmeta` fields this module does not interpret, kept verbatim.
pub type Extras = BTreeMap<String, serde_json::Value>;

/// The sidecar every list keeps.
pub const LISTMETA: &str = ".listmeta";

// ---------------------------------------------------------------------------
// The hash
// ---------------------------------------------------------------------------

pub fn leaf_hash(text: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"s\0");
    h.update(text.as_bytes());
    h.finalize().into()
}

pub fn branch_hash(text: &str, children: &[[u8; 32]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"o\0");
    h.update(text.as_bytes());
    h.update(b"\0");
    for c in children {
        h.update(c);
    }
    h.finalize().into()
}

pub fn root_hash(children: &[[u8; 32]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"r\0");
    for c in children {
        h.update(c);
    }
    h.finalize().into()
}

/// Lowercase hex, the form every hash is stored and shown in.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

/// A whole store: the top-level list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoreTree {
    /// The root `.listmeta`'s own extra fields.
    pub list_extra: Extras,
    pub children: Vec<StoreNode>,
}

/// One object: its text, and its children when it is a branch.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreNode {
    pub id: Id,
    pub text: String,
    /// Its entry's extra fields in its parent's `.listmeta` (the board's
    /// `archive`).
    pub child_extra: Extras,
    /// `Some` for a branch, even a childless one: a branch is somewhere the
    /// user can descend into, and hashes differently from a leaf.
    pub branch: Option<Branch>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Branch {
    /// Its own `.listmeta`'s extra fields (notes' `visibility`).
    pub list_extra: Extras,
    pub children: Vec<StoreNode>,
}

impl StoreNode {
    pub fn hash(&self) -> [u8; 32] {
        match &self.branch {
            Some(b) => branch_hash(&self.text, &digests(&b.children)),
            None => leaf_hash(&self.text),
        }
    }
}

impl StoreTree {
    pub fn root_hash(&self) -> [u8; 32] {
        root_hash(&digests(&self.children))
    }

    /// Every object's hash in hex, by id. One pass, bottom-up.
    pub fn hashes(&self) -> HashMap<Id, String> {
        fn walk(nodes: &[StoreNode], out: &mut HashMap<Id, String>) -> Vec<[u8; 32]> {
            nodes
                .iter()
                .map(|n| {
                    let d = match &n.branch {
                        Some(b) => branch_hash(&n.text, &walk(&b.children, out)),
                        None => leaf_hash(&n.text),
                    };
                    out.insert(n.id, hex(&d));
                    d
                })
                .collect()
        }
        let mut out = HashMap::new();
        walk(&self.children, &mut out);
        out
    }

    pub fn max_id(&self) -> Id {
        fn walk(nodes: &[StoreNode]) -> Id {
            nodes
                .iter()
                .map(|n| n.id.max(n.branch.as_ref().map_or(0, |b| walk(&b.children))))
                .max()
                .unwrap_or(0)
        }
        walk(&self.children)
    }
}

fn digests(nodes: &[StoreNode]) -> Vec<[u8; 32]> {
    nodes.iter().map(StoreNode::hash).collect()
}

// ---------------------------------------------------------------------------
// .listmeta
// ---------------------------------------------------------------------------

/// A list's sidecar. The field order is the bytes the plugins write
/// (`sha256`, then notes' `visibility`, then `children`), so a store a plugin
/// saved comes back from [`to_files`] unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListMeta {
    /// The hash of the object that owns this list, or the root hash at the
    /// top. Empty in a store written before it had hashes.
    #[serde(default)]
    pub sha256: String,
    #[serde(flatten)]
    pub extra: Extras,
    #[serde(default)]
    pub children: Vec<ChildMeta>,
}

/// One child's entry: its position, id and hash, so a peer can diff a subtree
/// without reading a single file's contents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChildMeta {
    /// 1-based position, matching the file name.
    pub n: usize,
    #[serde(default)]
    pub id: Id,
    #[serde(default)]
    pub sha256: String,
    #[serde(flatten)]
    pub extra: Extras,
}

fn entry_name(n: usize) -> String {
    format!("{n:04}")
}

fn parse_entry_name(name: &str) -> Option<usize> {
    if name.len() == 4 && name.bytes().all(|b| b.is_ascii_digit()) {
        name.parse().ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Files to tree, and back
// ---------------------------------------------------------------------------

/// Read a store's files (relative path to contents, as in a
/// [`crate::snapshot::Snapshot`]) into a tree.
///
/// As forgiving as the plugins' own loaders: a missing or unreadable
/// `.listmeta` leaves its ids unknown, a `.d` folder without a file of its own
/// is ignored, and an id that is missing or used twice is replaced by a fresh
/// one above every id in the store (in pre-order), so the result always has
/// unique, non-zero ids.
pub fn parse(files: &BTreeMap<String, String>) -> StoreTree {
    // Each directory's entries, keyed by the directory's prefix ("" or
    // "0001.d/0002.d/"), so a level is found without scanning every path.
    let mut dirs: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut branches: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for path in files.keys() {
        let (dir, name) = match path.rfind('/') {
            Some(i) => (&path[..=i], &path[i + 1..]),
            None => ("", path.as_str()),
        };
        if let Some(n) = parse_entry_name(name) {
            dirs.entry(dir).or_default().push(n);
        }
        // Every ancestor folder of a path makes its owner a branch.
        let mut end = 0;
        while let Some(i) = path[end..].find(".d/") {
            end += i + 3;
            branches.insert(&path[..end]);
        }
    }

    struct Ctx<'a> {
        files: &'a BTreeMap<String, String>,
        dirs: HashMap<&'a str, Vec<usize>>,
        branches: std::collections::HashSet<&'a str>,
    }

    fn list(ctx: &Ctx, prefix: &str) -> (Extras, Vec<StoreNode>) {
        let meta: ListMeta = ctx
            .files
            .get(&format!("{prefix}{LISTMETA}"))
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        let mut positions = ctx.dirs.get(prefix).cloned().unwrap_or_default();
        positions.sort_unstable();
        positions.dedup();
        let nodes = positions
            .into_iter()
            .map(|n| {
                let name = format!("{prefix}{}", entry_name(n));
                let child = meta.children.iter().find(|c| c.n == n);
                let sub = format!("{name}.d/");
                let branch = ctx.branches.contains(sub.as_str()).then(|| {
                    let (list_extra, children) = list(ctx, &sub);
                    Branch {
                        list_extra,
                        children,
                    }
                });
                StoreNode {
                    id: child.map_or(0, |c| c.id),
                    text: ctx.files.get(&name).cloned().unwrap_or_default(),
                    child_extra: child.map(|c| c.extra.clone()).unwrap_or_default(),
                    branch,
                }
            })
            .collect();
        (meta.extra, nodes)
    }

    let ctx = Ctx {
        files,
        dirs,
        branches,
    };
    let (list_extra, children) = list(&ctx, "");
    let mut tree = StoreTree {
        list_extra,
        children,
    };
    assign_missing_ids(&mut tree);
    tree
}

/// Replace every unknown (`0`) or repeated id with a fresh one, in pre-order,
/// above every id in the tree, so an id read off disk is never handed out
/// twice.
fn assign_missing_ids(tree: &mut StoreTree) {
    fn walk(nodes: &mut [StoreNode], seen: &mut std::collections::HashSet<Id>, next: &mut Id) {
        for n in nodes.iter_mut() {
            if n.id == 0 || !seen.insert(n.id) {
                n.id = *next;
                *next += 1;
                seen.insert(n.id);
            }
            if let Some(b) = &mut n.branch {
                walk(&mut b.children, seen, next);
            }
        }
    }
    let mut next = tree.max_id() + 1;
    walk(&mut tree.children, &mut Default::default(), &mut next);
}

/// Write a tree as a store's files, with every hash recomputed and positions
/// numbered densely from 1: the canonical form of the store.
pub fn to_files(tree: &StoreTree) -> BTreeMap<String, String> {
    fn emit(
        prefix: &str,
        list_extra: &Extras,
        nodes: &[StoreNode],
        own: &dyn Fn(&[[u8; 32]]) -> [u8; 32],
        out: &mut BTreeMap<String, String>,
    ) -> [u8; 32] {
        let mut digests = Vec::with_capacity(nodes.len());
        let mut children = Vec::with_capacity(nodes.len());
        for (i, node) in nodes.iter().enumerate() {
            let n = i + 1;
            let name = format!("{prefix}{}", entry_name(n));
            out.insert(name.clone(), node.text.clone());
            let d = match &node.branch {
                Some(b) => emit(
                    &format!("{name}.d/"),
                    &b.list_extra,
                    &b.children,
                    &|ds| branch_hash(&node.text, ds),
                    out,
                ),
                None => leaf_hash(&node.text),
            };
            children.push(ChildMeta {
                n,
                id: node.id,
                sha256: hex(&d),
                extra: node.child_extra.clone(),
            });
            digests.push(d);
        }
        let own = own(&digests);
        let meta = ListMeta {
            sha256: hex(&own),
            extra: list_extra.clone(),
            children,
        };
        out.insert(
            format!("{prefix}{LISTMETA}"),
            serde_json::to_string_pretty(&meta).unwrap_or_default(),
        );
        own
    }
    let mut out = BTreeMap::new();
    emit("", &tree.list_extra, &tree.children, &root_hash, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Verify
// ---------------------------------------------------------------------------

/// What [`verify`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verified {
    /// Every hash the store carries is the hash of what it holds.
    Ok,
    /// The store carries no hashes at all: written before it had them (a
    /// board from before 0.4, a board the Trello script wrote). Nothing to
    /// check, and [`to_files`] fills them in.
    Legacy,
    /// The deepest `.listmeta` (or a list missing one) whose hashes do not
    /// match what the store holds, which is where the change is (every
    /// ancestor's is stale too): a store read half-saved, or edited by hand.
    Mismatch { path: String },
}

/// Check the hashes a store's files carry against what they hold.
pub fn verify(files: &BTreeMap<String, String>) -> Verified {
    let carried: BTreeMap<&String, ListMeta> = files
        .iter()
        .filter(|(p, _)| p.rsplit('/').next() == Some(LISTMETA))
        .map(|(p, raw)| (p, serde_json::from_str(raw).unwrap_or_default()))
        .collect();
    let any_hash = carried
        .values()
        .any(|m| !m.sha256.is_empty() || m.children.iter().any(|c| !c.sha256.is_empty()));
    if !any_hash {
        return Verified::Legacy;
    }
    let canonical = to_files(&parse(files));
    // Reversed, a folder's `.listmeta` comes before its parent's.
    for (path, raw) in canonical.iter().rev() {
        if path.rsplit('/').next() != Some(LISTMETA) {
            continue;
        }
        let want: ListMeta = serde_json::from_str(raw).unwrap_or_default();
        let ok = carried.get(path).is_some_and(|have| {
            have.sha256 == want.sha256
                && have.children.len() == want.children.len()
                && have
                    .children
                    .iter()
                    .zip(&want.children)
                    .all(|(h, w)| h.n == w.n && h.sha256 == w.sha256)
        });
        if !ok {
            return Verified::Mismatch { path: path.clone() };
        }
    }
    Verified::Ok
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

/// How an object differs between two trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Only in the second tree.
    Added,
    /// Only in the first tree.
    Removed,
    /// Its own text, or whether it is a branch, changed.
    Content,
    /// It now sits under another parent.
    Moved,
    /// Its children were reordered (the id is the parent's, `0` for the top).
    Reordered,
    /// Only fields outside the hash changed (visibility, archive).
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Change {
    pub id: Id,
    pub kind: ChangeKind,
}

/// Which objects differ between `a` and `b`, by id, in pre-order of `b` (then
/// the removed ones). Subtrees whose hash, ids and extras are all equal are
/// skipped: that is what the Merkle hashes are for.
pub fn diff(a: &StoreTree, b: &StoreTree) -> Vec<Change> {
    let fa = Flat::of(a);
    let fb = Flat::of(b);
    let mut out = Vec::new();

    fn walk(
        nodes: &[StoreNode],
        parent: Id,
        fa: &Flat,
        out: &mut Vec<Change>,
        same: &dyn Fn(Id) -> bool,
    ) {
        if let Some(old) = fa.order.get(&parent) {
            let now: Vec<Id> = nodes.iter().map(|n| n.id).collect();
            let kept_old: Vec<Id> = old.iter().copied().filter(|i| now.contains(i)).collect();
            let kept_now: Vec<Id> = now.iter().copied().filter(|i| old.contains(i)).collect();
            if kept_old != kept_now {
                out.push(Change {
                    id: parent,
                    kind: ChangeKind::Reordered,
                });
            }
        }
        for n in nodes {
            if same(n.id) {
                continue;
            }
            match fa.recs.get(&n.id) {
                None => out.push(Change {
                    id: n.id,
                    kind: ChangeKind::Added,
                }),
                Some(old) => {
                    let kind = if old.parent != parent {
                        Some(ChangeKind::Moved)
                    } else if old.text != n.text || old.branch != n.branch.is_some() {
                        Some(ChangeKind::Content)
                    } else if old.child_extra != n.child_extra
                        || old.list_extra
                            != n.branch
                                .as_ref()
                                .map(|b| b.list_extra.clone())
                                .unwrap_or_default()
                    {
                        Some(ChangeKind::Meta)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        out.push(Change { id: n.id, kind });
                    }
                }
            }
            if let Some(br) = &n.branch {
                walk(&br.children, n.id, fa, out, same);
            }
        }
    }

    // A subtree is the same when its hash matches, and every id and extra
    // inside it does too (`sig` covers those, the hash covers the content).
    let same = |id: Id| -> bool {
        match (
            fa.subtree.get(&id),
            fb.subtree.get(&id),
            fa.recs.get(&id),
            fb.recs.get(&id),
        ) {
            (Some(x), Some(y), Some(ra), Some(rb)) => x == y && ra.parent == rb.parent,
            _ => false,
        }
    };
    if a.list_extra != b.list_extra {
        out.push(Change {
            id: 0,
            kind: ChangeKind::Meta,
        });
    }
    walk(&b.children, 0, &fa, &mut out, &same);
    for id in &fa.preorder {
        if !fb.recs.contains_key(id) {
            out.push(Change {
                id: *id,
                kind: ChangeKind::Removed,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// A tree flattened by id, for the diff and the merge
// ---------------------------------------------------------------------------

/// One object without its children: what the merge compares field by field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rec {
    /// `0` for a top-level object.
    pub parent: Id,
    pub text: String,
    pub branch: bool,
    pub child_extra: Extras,
    pub list_extra: Extras,
}

#[derive(Debug, Default)]
pub(crate) struct Flat {
    pub recs: HashMap<Id, Rec>,
    /// Each parent's children in order (`0`: the top level).
    pub order: HashMap<Id, Vec<Id>>,
    pub preorder: Vec<Id>,
    pub root_extra: Extras,
    /// A signature of each object's whole subtree: its hash, plus every id and
    /// extra inside it, so equal signatures mean nothing below differs.
    pub subtree: HashMap<Id, [u8; 32]>,
}

impl Flat {
    pub(crate) fn of(tree: &StoreTree) -> Flat {
        fn walk(nodes: &[StoreNode], parent: Id, flat: &mut Flat) -> Vec<[u8; 32]> {
            flat.order
                .insert(parent, nodes.iter().map(|n| n.id).collect());
            nodes
                .iter()
                .map(|n| {
                    flat.preorder.push(n.id);
                    let (kids, list_extra) = match &n.branch {
                        Some(b) => (walk(&b.children, n.id, flat), b.list_extra.clone()),
                        None => (Vec::new(), Extras::new()),
                    };
                    let mut h = Sha256::new();
                    h.update(n.hash());
                    h.update(n.id.to_le_bytes());
                    h.update(serde_json::to_vec(&n.child_extra).unwrap_or_default());
                    h.update(serde_json::to_vec(&list_extra).unwrap_or_default());
                    for k in &kids {
                        h.update(k);
                    }
                    let sig: [u8; 32] = h.finalize().into();
                    flat.subtree.insert(n.id, sig);
                    flat.recs.insert(
                        n.id,
                        Rec {
                            parent,
                            text: n.text.clone(),
                            branch: n.branch.is_some(),
                            child_extra: n.child_extra.clone(),
                            list_extra,
                        },
                    );
                    sig
                })
                .collect()
        }
        let mut flat = Flat {
            root_extra: tree.list_extra.clone(),
            ..Flat::default()
        };
        walk(&tree.children, 0, &mut flat);
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn leaf(id: Id, text: &str) -> StoreNode {
        StoreNode {
            id,
            text: text.to_owned(),
            child_extra: Extras::new(),
            branch: None,
        }
    }

    pub(crate) fn branch(id: Id, text: &str, children: Vec<StoreNode>) -> StoreNode {
        StoreNode {
            id,
            text: text.to_owned(),
            child_extra: Extras::new(),
            branch: Some(Branch {
                list_extra: Extras::new(),
                children,
            }),
        }
    }

    pub(crate) fn tree(children: Vec<StoreNode>) -> StoreTree {
        StoreTree {
            list_extra: Extras::new(),
            children,
        }
    }

    /// Groceries [milk, Weekend [bread]], Ideas: the notes vector.
    fn notes() -> StoreTree {
        tree(vec![
            branch(
                1,
                "Groceries",
                vec![
                    leaf(2, "milk"),
                    branch(3, "Weekend", vec![leaf(4, "bread")]),
                ],
            ),
            leaf(5, "Ideas"),
        ])
    }

    /// To do [fix login, write docs], Doing [kanban ui], Archive []: the board
    /// vector, with the archive flag on the last column.
    fn board() -> StoreTree {
        let mut archive = branch(7, "Archive", vec![]);
        archive
            .child_extra
            .insert("archive".to_owned(), json!(true));
        tree(vec![
            branch(
                1,
                "To do",
                vec![leaf(2, "fix login"), leaf(3, "write docs")],
            ),
            branch(4, "Doing", vec![leaf(5, "kanban ui")]),
            archive,
        ])
    }

    // ---- the hash ---------------------------------------------------------

    /// Pinned to bytes computed independently (Python's hashlib over the
    /// formula in the module doc). The notes plugin asserts the same vector.
    #[test]
    fn the_hashes_match_the_published_vectors() {
        let t = notes();
        assert_eq!(
            hex(&t.children[0].hash()),
            "7f659c2de7c765c2d0ef2d8f16aa1ae315c7f071e74bcfa6419fdcbb1a99a25d"
        );
        assert_eq!(
            hex(&t.root_hash()),
            "02b439fedb3ec6dd9b403bfa49e40bf5eb3fdcacd8bf3f501eca3278d98f79ea"
        );
        let b = board();
        assert_eq!(
            hex(&b.children[0].hash()),
            "1de2213ad66453d1d5afab7d344604da9da19f0e0c55dcc6dfa9e59470b0fd26"
        );
        assert_eq!(
            hex(&b.root_hash()),
            "ca4e849aee0589b69f232b07c8d6fed5a793ec7e412738e6dd9c156f0650ef40"
        );
        assert_eq!(
            hex(&StoreTree::default().root_hash()),
            "96229c0a1dcb79d7d50913f882e3144961b5616140ded9ab844bd685e08e3a30"
        );
    }

    #[test]
    fn ids_and_extras_leave_the_hash_alone() {
        let mut t = board();
        let before = t.root_hash();
        t.children[0].id = 99;
        t.children[2].child_extra.clear();
        t.children[1]
            .branch
            .as_mut()
            .unwrap()
            .list_extra
            .insert("visibility".to_owned(), json!("public"));
        assert_eq!(t.root_hash(), before);
    }

    #[test]
    fn a_leaf_and_an_empty_branch_hash_differently() {
        assert_ne!(leaf(1, "x").hash(), branch(1, "x", vec![]).hash());
    }

    #[test]
    fn hashes_names_every_object() {
        let t = notes();
        let h = t.hashes();
        assert_eq!(h.len(), 5);
        assert_eq!(h[&1], hex(&t.children[0].hash()));
        assert_eq!(h[&4], hex(&leaf_hash("bread")));
    }

    // ---- files ------------------------------------------------------------

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// The bytes the notes plugin writes (`serde_json::to_string_pretty` of
    /// its own `ListMeta`) come back unchanged: field order included, which is
    /// where `#[serde(flatten)]` puts `visibility`.
    #[test]
    fn a_notes_store_round_trips_byte_for_byte() {
        let mut t = notes();
        t.children[0]
            .branch
            .as_mut()
            .unwrap()
            .list_extra
            .insert("visibility".to_owned(), json!("private"));
        let written = to_files(&t);
        assert_eq!(
            written["0001.d/.listmeta"],
            format!(
                "{{\n  \"sha256\": \"{}\",\n  \"visibility\": \"private\",\n  \"children\": [\n    {{\n      \"n\": 1,\n      \"id\": 2,\n      \"sha256\": \"{}\"\n    }},\n    {{\n      \"n\": 2,\n      \"id\": 3,\n      \"sha256\": \"{}\"\n    }}\n  ]\n}}",
                hex(&t.children[0].hash()),
                hex(&leaf_hash("milk")),
                hex(&t.children[0].branch.as_ref().unwrap().children[1].hash()),
            )
        );
        assert_eq!(parse(&written), t);
        assert_eq!(to_files(&parse(&written)), written);
        assert_eq!(verify(&written), Verified::Ok);
    }

    #[test]
    fn a_board_round_trips_with_its_archive_flag_after_the_hash() {
        let b = board();
        let written = to_files(&b);
        let root = &written[".listmeta"];
        let archive_entry = root.rfind("\"n\": 3").unwrap();
        let sha = root[archive_entry..].find("\"sha256\"").unwrap();
        let flag = root[archive_entry..].find("\"archive\": true").unwrap();
        assert!(sha < flag, "{root}");
        assert_eq!(parse(&written), b);
        assert_eq!(verify(&written), Verified::Ok);
        // An empty column is still a branch: its folder holds a `.listmeta`.
        assert!(written.contains_key("0003.d/.listmeta"));
    }

    /// A board saved before it had hashes: no `sha256` anywhere.
    #[test]
    fn a_store_without_hashes_is_legacy_and_gains_them() {
        let legacy = files(&[
            (
                ".listmeta",
                r#"{"children":[{"n":1,"id":1},{"n":2,"id":4,"archive":true}]}"#,
            ),
            ("0001", "To do"),
            ("0001.d/.listmeta", r#"{"children":[{"n":1,"id":2}]}"#),
            ("0001.d/0001", "fix login"),
            ("0002", "Archive"),
            ("0002.d/.listmeta", r#"{"children":[]}"#),
        ]);
        assert_eq!(verify(&legacy), Verified::Legacy);
        let t = parse(&legacy);
        assert_eq!(t.children[1].child_extra["archive"], json!(true));
        let canonical = to_files(&t);
        assert_eq!(verify(&canonical), Verified::Ok);
        assert_eq!(parse(&canonical), t);
    }

    #[test]
    fn a_stale_hash_is_a_mismatch_at_its_list() {
        let mut written = to_files(&notes());
        written.insert("0001.d/0001".to_owned(), "oat milk".to_owned());
        assert_eq!(
            verify(&written),
            Verified::Mismatch {
                path: "0001.d/.listmeta".to_owned()
            }
        );
    }

    #[test]
    fn missing_and_repeated_ids_get_fresh_ones_above_the_rest() {
        let f = files(&[
            (
                ".listmeta",
                r#"{"children":[{"n":1,"id":5},{"n":2,"id":5}]}"#,
            ),
            ("0001", "a"),
            ("0002", "b"),
            ("0003", "c"),
        ]);
        let t = parse(&f);
        let ids: Vec<Id> = t.children.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![5, 6, 7]);
    }

    #[test]
    fn junk_listmeta_and_orphan_folders_are_forgiven() {
        let f = files(&[
            (".listmeta", "not json"),
            ("0001", "a"),
            ("0007.d/0001", "orphan"),
        ]);
        let t = parse(&f);
        assert_eq!(t.children.len(), 1);
        assert_eq!(t.children[0].text, "a");
        assert!(t.children[0].branch.is_none());
    }

    #[test]
    fn an_empty_store_is_an_empty_tree() {
        let t = parse(&BTreeMap::new());
        assert_eq!(t, StoreTree::default());
        assert_eq!(to_files(&t).len(), 1, "the root .listmeta");
    }

    // ---- diff -------------------------------------------------------------

    #[test]
    fn equal_trees_have_no_diff() {
        assert!(diff(&board(), &board()).is_empty());
    }

    #[test]
    fn an_edit_a_move_a_flag_and_a_reorder_are_each_named() {
        let a = board();
        let mut b = board();
        // Edit "write docs".
        b.children[0].branch.as_mut().unwrap().children[1].text = "write the docs".into();
        // Move "fix login" to Doing.
        let card = b.children[0].branch.as_mut().unwrap().children.remove(0);
        b.children[1].branch.as_mut().unwrap().children.push(card);
        // Un-archive the archive.
        b.children[2].child_extra.clear();
        // Swap the first two columns.
        b.children.swap(0, 1);

        let changes = diff(&a, &b);
        let has = |id, kind| changes.contains(&Change { id, kind });
        assert!(has(3, ChangeKind::Content), "{changes:?}");
        assert!(has(2, ChangeKind::Moved), "{changes:?}");
        assert!(has(7, ChangeKind::Meta), "{changes:?}");
        assert!(has(0, ChangeKind::Reordered), "{changes:?}");
        assert!(!changes.iter().any(|c| c.id == 5), "kanban ui is untouched");
    }

    #[test]
    fn added_and_removed_objects_are_named() {
        let a = notes();
        let mut b = notes();
        b.children.remove(1);
        b.children.push(leaf(9, "new"));
        let changes = diff(&a, &b);
        assert!(changes.contains(&Change {
            id: 9,
            kind: ChangeKind::Added
        }));
        assert!(changes.contains(&Change {
            id: 5,
            kind: ChangeKind::Removed
        }));
    }
}
