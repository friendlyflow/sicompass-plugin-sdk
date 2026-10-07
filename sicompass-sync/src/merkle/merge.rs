//! The three-way merge a sync runs when both this machine and the cloud
//! changed since they last agreed.
//!
//! `base` is the tree both sides last agreed on, `local` this machine's,
//! `remote` the cloud's. Objects are matched by id, and each one's fields
//! (parent, text, branch or leaf, its extras) are merged separately, so a card
//! moved on one machine and edited on the other ends up moved *and* edited.
//!
//! - A field only one side changed takes that side's value.
//! - A field both sides changed, differently, takes the `prefer` side's value
//!   (the sync passes whichever changed last), and counts as a conflict.
//! - An object deleted on one side and left alone on the other is deleted. One
//!   deleted on one side and changed on the other is kept: a merge never loses
//!   an edit. Its parent comes back too if it had gone.
//! - An id both sides added with different contents is two objects, not one:
//!   both machines minted the same number for different things. The local one
//!   gets a fresh id ([`Merged::reminted`]). Without a base, every shared id is
//!   such a case, so nothing is lost and at worst something is kept twice.
//! - A list's order is the order of the side that reordered it (`prefer` when
//!   both did), with what the other side inserted placed after the object it
//!   followed there.

use std::collections::{HashMap, HashSet};

use super::{Branch, Extras, Flat, Id, Rec, StoreNode, StoreTree};

/// Which side wins a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub tree: StoreTree,
    /// Local ids that collided with a different remote object, and the ids
    /// they were given instead.
    pub reminted: Vec<(Id, Id)>,
    /// Fields both sides changed differently, settled by `prefer`.
    pub conflicts: usize,
}

/// Merge `local` and `remote` against the tree they last agreed on.
pub fn merge3(
    base: Option<&StoreTree>,
    local: &StoreTree,
    remote: &StoreTree,
    prefer: Side,
) -> Merged {
    let fb = base.map(Flat::of).unwrap_or_default();
    let mut fl = Flat::of(local);
    let fr = Flat::of(remote);

    // ---- collisions: the same new id for different objects ------------------
    let next = local
        .max_id()
        .max(remote.max_id())
        .max(base.map_or(0, StoreTree::max_id))
        + 1;
    let mut reminted = Vec::new();
    let mut colliding: Vec<Id> = fl
        .recs
        .iter()
        .filter(|(id, l)| !fb.recs.contains_key(id) && fr.recs.get(id).is_some_and(|r| r != *l))
        .map(|(id, _)| *id)
        .collect();
    // Pre-order, so a parent is renumbered before its children read it.
    colliding.sort_by_key(|id| fl.preorder.iter().position(|p| p == id));
    for (old, new) in colliding.into_iter().zip(next..) {
        remint(&mut fl, old, new);
        reminted.push((old, new));
    }

    // ---- each object, field by field ---------------------------------------
    let mut conflicts = 0;
    let mut kept: HashMap<Id, Rec> = HashMap::new();
    let mut ids: Vec<Id> = fl.preorder.clone();
    ids.extend(fr.preorder.iter().filter(|id| !fl.recs.contains_key(id)));
    for id in &ids {
        let b = fb.recs.get(id);
        let rec = match (fl.recs.get(id), fr.recs.get(id)) {
            (Some(l), Some(r)) => merge_rec(b, l, r, prefer, &mut conflicts),
            // Deleted remotely: gone, unless this machine changed it since.
            (Some(l), None) => match b {
                Some(b) if !changed(b, l) => continue,
                _ => l.clone(),
            },
            (None, Some(r)) => match b {
                Some(b) if !changed(b, r) => continue,
                _ => r.clone(),
            },
            (None, None) => continue,
        };
        kept.insert(*id, rec);
    }

    // ---- parents a kept object still needs ----------------------------------
    let mut i = 0;
    let mut queue: Vec<Id> = kept.keys().copied().collect();
    queue.sort_unstable();
    while i < queue.len() {
        let parent = kept[&queue[i]].parent;
        i += 1;
        if parent == 0 || kept.contains_key(&parent) {
            continue;
        }
        let back = fl
            .recs
            .get(&parent)
            .or_else(|| fr.recs.get(&parent))
            .or_else(|| fb.recs.get(&parent))
            .cloned();
        match back {
            Some(rec) => {
                kept.insert(parent, rec);
                queue.push(parent);
            }
            // Nowhere to be found: hang it from the top rather than lose it.
            None => {
                let child = queue[i - 1];
                if let Some(c) = kept.get_mut(&child) {
                    c.parent = 0;
                }
            }
        }
    }
    // A parent that came back must be a branch to hold its children.
    let parents: HashSet<Id> = kept.values().map(|r| r.parent).collect();
    for (id, rec) in kept.iter_mut() {
        if parents.contains(id) {
            rec.branch = true;
        }
    }

    // ---- cycles: A under B on one side, B under A on the other --------------
    let ids: Vec<Id> = kept.keys().copied().collect();
    for id in ids {
        let mut seen = HashSet::new();
        let mut cur = id;
        while cur != 0 {
            if !seen.insert(cur) {
                // Hang the object that closed the loop from the top.
                if let Some(r) = kept.get_mut(&cur) {
                    r.parent = 0;
                }
                break;
            }
            cur = kept.get(&cur).map_or(0, |r| r.parent);
        }
    }

    // ---- order, and the tree ------------------------------------------------
    let mut children: HashMap<Id, HashSet<Id>> = HashMap::new();
    for (id, rec) in &kept {
        children.entry(rec.parent).or_default().insert(*id);
    }
    let order_of = |parent: Id| -> Vec<Id> {
        let members = children.get(&parent).cloned().unwrap_or_default();
        merge_order(
            fb.order.get(&parent),
            fl.order.get(&parent),
            fr.order.get(&parent),
            prefer,
            &members,
            &fl.preorder,
            &fr.preorder,
        )
    };

    fn build(
        parent: Id,
        kept: &HashMap<Id, Rec>,
        order_of: &dyn Fn(Id) -> Vec<Id>,
    ) -> Vec<StoreNode> {
        order_of(parent)
            .into_iter()
            .map(|id| {
                let rec = &kept[&id];
                StoreNode {
                    id,
                    text: rec.text.clone(),
                    child_extra: rec.child_extra.clone(),
                    branch: rec.branch.then(|| Branch {
                        list_extra: rec.list_extra.clone(),
                        children: build(id, kept, order_of),
                    }),
                }
            })
            .collect()
    }

    let root_extra = merge_field(
        base.map(|_| &fb.root_extra),
        &fl.root_extra,
        &fr.root_extra,
        prefer,
        &mut conflicts,
    );
    Merged {
        tree: StoreTree {
            list_extra: root_extra,
            children: build(0, &kept, &order_of),
        },
        reminted,
        conflicts,
    }
}

/// Whether `now` differs from `then` in anything but its children.
fn changed(then: &Rec, now: &Rec) -> bool {
    then != now
}

fn merge_rec(b: Option<&Rec>, l: &Rec, r: &Rec, prefer: Side, conflicts: &mut usize) -> Rec {
    Rec {
        parent: merge_field(
            b.map(|b| &b.parent),
            &l.parent,
            &r.parent,
            prefer,
            conflicts,
        ),
        text: merge_field(b.map(|b| &b.text), &l.text, &r.text, prefer, conflicts),
        branch: merge_field(
            b.map(|b| &b.branch),
            &l.branch,
            &r.branch,
            prefer,
            conflicts,
        ),
        child_extra: merge_extras(
            b.map(|b| &b.child_extra),
            &l.child_extra,
            &r.child_extra,
            prefer,
            conflicts,
        ),
        list_extra: merge_extras(
            b.map(|b| &b.list_extra),
            &l.list_extra,
            &r.list_extra,
            prefer,
            conflicts,
        ),
    }
}

fn merge_field<T: Clone + PartialEq>(
    base: Option<&T>,
    local: &T,
    remote: &T,
    prefer: Side,
    conflicts: &mut usize,
) -> T {
    if local == remote {
        return local.clone();
    }
    match base {
        Some(b) if b == local => remote.clone(),
        Some(b) if b == remote => local.clone(),
        _ => {
            *conflicts += 1;
            match prefer {
                Side::Local => local.clone(),
                Side::Remote => remote.clone(),
            }
        }
    }
}

/// Extras merge key by key, so archiving a column on one machine and
/// publishing a note on the other do not collide.
fn merge_extras(
    base: Option<&Extras>,
    local: &Extras,
    remote: &Extras,
    prefer: Side,
    conflicts: &mut usize,
) -> Extras {
    let keys: HashSet<&String> = local.keys().chain(remote.keys()).collect();
    let mut out = Extras::new();
    for key in keys {
        let merged = merge_field(
            base.map(|b| b.get(key)).as_ref(),
            &local.get(key),
            &remote.get(key),
            prefer,
            conflicts,
        );
        if let Some(v) = merged {
            out.insert(key.clone(), v.clone());
        }
    }
    out
}

/// The order of one list. `members` are the ids that end up in it.
fn merge_order(
    base: Option<&Vec<Id>>,
    local: Option<&Vec<Id>>,
    remote: Option<&Vec<Id>>,
    prefer: Side,
    members: &HashSet<Id>,
    local_all: &[Id],
    remote_all: &[Id],
) -> Vec<Id> {
    let empty = Vec::new();
    let (b, l, r) = (
        base.unwrap_or(&empty),
        local.unwrap_or(&empty),
        remote.unwrap_or(&empty),
    );
    // Did a side reorder what all three share?
    let common: HashSet<Id> = b
        .iter()
        .filter(|id| l.contains(id) && r.contains(id))
        .copied()
        .collect();
    let only =
        |v: &Vec<Id>| -> Vec<Id> { v.iter().copied().filter(|i| common.contains(i)).collect() };
    let (primary, secondary) = if only(l) == only(b) {
        (r, l)
    } else if only(r) == only(b) {
        (l, r)
    } else {
        match prefer {
            Side::Local => (l, r),
            Side::Remote => (r, l),
        }
    };

    let mut out: Vec<Id> = primary
        .iter()
        .copied()
        .filter(|i| members.contains(i))
        .collect();
    // What only the other side has goes after what it followed there.
    for (pos, id) in secondary.iter().enumerate() {
        if !members.contains(id) || out.contains(id) {
            continue;
        }
        let after = secondary[..pos].iter().rev().find(|p| out.contains(p));
        let at = after.map_or(0, |p| out.iter().position(|o| o == p).unwrap() + 1);
        out.insert(at, *id);
    }
    // Moved in from another list, or brought back from the base: in the order
    // the sides list them overall.
    let mut rest: Vec<Id> = members
        .iter()
        .copied()
        .filter(|i| !out.contains(i))
        .collect();
    rest.sort_by_key(|id| {
        let l = local_all.iter().position(|x| x == id);
        let r = remote_all.iter().position(|x| x == id);
        (l.or(r).unwrap_or(usize::MAX), *id)
    });
    out.extend(rest);
    out
}

/// Give local object `old` the id `new`, everywhere the flat tree names it.
fn remint(flat: &mut Flat, old: Id, new: Id) {
    if let Some(rec) = flat.recs.remove(&old) {
        flat.recs.insert(new, rec);
    }
    for rec in flat.recs.values_mut() {
        if rec.parent == old {
            rec.parent = new;
        }
    }
    if let Some(kids) = flat.order.remove(&old) {
        flat.order.insert(new, kids);
    }
    for list in flat.order.values_mut() {
        for id in list.iter_mut() {
            if *id == old {
                *id = new;
            }
        }
    }
    for id in flat.preorder.iter_mut() {
        if *id == old {
            *id = new;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{branch, leaf, tree};
    use super::super::{Verified, to_files, verify};
    use super::*;
    use serde_json::json;

    /// To do [a, b], Doing [c].
    fn base() -> StoreTree {
        tree(vec![
            branch(1, "To do", vec![leaf(2, "a"), leaf(3, "b")]),
            branch(4, "Doing", vec![leaf(5, "c")]),
        ])
    }

    fn col(t: &mut StoreTree, i: usize) -> &mut Vec<StoreNode> {
        &mut t.children[i].branch.as_mut().unwrap().children
    }

    fn texts(nodes: &[StoreNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.text.as_str()).collect()
    }

    fn sound(m: &Merged) {
        assert_eq!(verify(&to_files(&m.tree)), Verified::Ok);
        let mut ids = Vec::new();
        fn walk(n: &[StoreNode], ids: &mut Vec<Id>) {
            for x in n {
                ids.push(x.id);
                if let Some(b) = &x.branch {
                    walk(&b.children, ids);
                }
            }
        }
        walk(&m.tree.children, &mut ids);
        let unique: HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "duplicate ids: {ids:?}");
    }

    fn merge(l: &StoreTree, r: &StoreTree, prefer: Side) -> Merged {
        let m = merge3(Some(&base()), l, r, prefer);
        sound(&m);
        m
    }

    #[test]
    fn nothing_changed_is_the_base() {
        let m = merge(&base(), &base(), Side::Local);
        assert_eq!(m.tree, base());
        assert_eq!(m.conflicts, 0);
    }

    #[test]
    fn a_change_on_one_side_is_taken_from_either_side() {
        let mut l = base();
        col(&mut l, 0)[0].text = "a!".into();
        let mut r = base();
        col(&mut r, 1)[0].text = "c!".into();
        for prefer in [Side::Local, Side::Remote] {
            let mut m = merge(&l, &r, prefer);
            assert_eq!(texts(col(&mut m.tree, 0)), vec!["a!", "b"]);
            assert_eq!(texts(col(&mut m.tree, 1)), vec!["c!"]);
            assert_eq!(m.conflicts, 0);
        }
    }

    #[test]
    fn both_changing_one_field_goes_to_the_preferred_side() {
        let mut l = base();
        col(&mut l, 0)[0].text = "local".into();
        let mut r = base();
        col(&mut r, 0)[0].text = "remote".into();
        let mut m = merge(&l, &r, Side::Remote);
        assert_eq!(col(&mut m.tree, 0)[0].text, "remote");
        assert_eq!(m.conflicts, 1);
        let mut m = merge(&l, &r, Side::Local);
        assert_eq!(col(&mut m.tree, 0)[0].text, "local");
    }

    #[test]
    fn the_same_change_on_both_sides_is_no_conflict() {
        let mut l = base();
        col(&mut l, 0)[0].text = "same".into();
        let m = merge(&l, &l.clone(), Side::Local);
        assert_eq!(m.conflicts, 0);
        assert_eq!(m.tree, l);
    }

    #[test]
    fn a_delete_against_an_untouched_object_deletes_it() {
        let mut l = base();
        col(&mut l, 0).remove(1);
        let mut m = merge(&l, &base(), Side::Remote);
        assert_eq!(texts(col(&mut m.tree, 0)), vec!["a"]);
    }

    #[test]
    fn a_delete_against_an_edit_keeps_the_edit() {
        let mut l = base();
        col(&mut l, 0).remove(1);
        let mut r = base();
        col(&mut r, 0)[1].text = "b edited".into();
        let mut m = merge(&l, &r, Side::Local);
        assert_eq!(texts(col(&mut m.tree, 0)), vec!["a", "b edited"]);
    }

    #[test]
    fn a_deleted_list_comes_back_for_a_card_added_to_it() {
        let mut l = base();
        l.children.remove(1); // Doing, and c, deleted here
        let mut r = base();
        col(&mut r, 1).push(leaf(9, "d")); // a card added to Doing there
        let mut m = merge(&l, &r, Side::Local);
        assert_eq!(texts(&m.tree.children), vec!["To do", "Doing"]);
        // c was untouched remotely, so its delete stands; d is kept.
        assert_eq!(texts(col(&mut m.tree, 1)), vec!["d"]);
    }

    #[test]
    fn a_card_moved_here_and_edited_there_is_moved_and_edited() {
        let mut l = base();
        let card = col(&mut l, 0).remove(0);
        col(&mut l, 1).push(card);
        let mut r = base();
        col(&mut r, 0)[0].text = "a edited".into();
        let mut m = merge(&l, &r, Side::Remote);
        assert_eq!(texts(col(&mut m.tree, 0)), vec!["b"]);
        assert_eq!(texts(col(&mut m.tree, 1)), vec!["c", "a edited"]);
        assert_eq!(m.conflicts, 0);
    }

    #[test]
    fn a_reorder_here_and_an_insert_there_keeps_both() {
        let mut l = base();
        col(&mut l, 0).swap(0, 1); // [b, a]
        let mut r = base();
        col(&mut r, 0).insert(1, leaf(9, "between")); // [a, between, b]
        let mut m = merge(&l, &r, Side::Remote);
        assert_eq!(texts(col(&mut m.tree, 0)), vec!["b", "a", "between"]);
    }

    #[test]
    fn colliding_new_ids_are_two_objects_and_the_local_one_moves() {
        let mut l = base();
        col(&mut l, 0).push(leaf(9, "mine"));
        let mut r = base();
        col(&mut r, 1).push(leaf(9, "theirs"));
        let mut m = merge(&l, &r, Side::Remote);
        assert_eq!(m.reminted, vec![(9, 10)]);
        assert_eq!(texts(col(&mut m.tree, 0)), vec!["a", "b", "mine"]);
        assert_eq!(col(&mut m.tree, 0)[2].id, 10);
        assert_eq!(texts(col(&mut m.tree, 1)), vec!["c", "theirs"]);
        assert_eq!(col(&mut m.tree, 1)[1].id, 9);
    }

    #[test]
    fn a_colliding_list_takes_its_cards_along_to_the_new_id() {
        let mut l = base();
        l.children.push(branch(9, "Mine", vec![leaf(10, "x")]));
        let mut r = base();
        r.children.push(branch(9, "Theirs", vec![leaf(10, "y")]));
        let m = merge(&l, &r, Side::Local);
        let mine = m.tree.children.iter().find(|c| c.text == "Mine").unwrap();
        let theirs = m.tree.children.iter().find(|c| c.text == "Theirs").unwrap();
        assert_ne!(mine.id, theirs.id);
        assert_eq!(texts(&mine.branch.as_ref().unwrap().children), vec!["x"]);
        assert_eq!(texts(&theirs.branch.as_ref().unwrap().children), vec!["y"]);
    }

    #[test]
    fn the_same_object_added_on_both_sides_is_kept_once() {
        let mut l = base();
        col(&mut l, 0).push(leaf(9, "same"));
        let r = l.clone();
        let m = merge(&l, &r, Side::Local);
        assert!(m.reminted.is_empty());
        assert_eq!(m.tree, l);
    }

    #[test]
    fn without_a_base_nothing_is_lost() {
        let mut l = base();
        col(&mut l, 0)[0].text = "a here".into();
        let mut r = base();
        col(&mut r, 0)[0].text = "a there".into();
        let mut m = merge3(None, &l, &r, Side::Local);
        sound(&m);
        let todo = texts(col(&mut m.tree, 0));
        assert!(
            todo.contains(&"a here") && todo.contains(&"a there"),
            "{todo:?}"
        );
        // What both had alike is not doubled.
        assert_eq!(todo.iter().filter(|t| **t == "b").count(), 1);
    }

    #[test]
    fn extras_merge_key_by_key() {
        let mut l = base();
        l.children[1]
            .child_extra
            .insert("archive".to_owned(), json!(true));
        let mut r = base();
        r.children[0]
            .branch
            .as_mut()
            .unwrap()
            .list_extra
            .insert("visibility".to_owned(), json!("public"));
        col(&mut r, 1)[0].text = "c!".into();
        let m = merge(&l, &r, Side::Remote);
        assert_eq!(m.tree.children[1].child_extra["archive"], json!(true));
        assert_eq!(
            m.tree.children[0].branch.as_ref().unwrap().list_extra["visibility"],
            json!("public")
        );
        assert_eq!(m.conflicts, 0);
    }

    #[test]
    fn crossed_moves_cannot_make_a_loop() {
        // Notes-shaped: X and Y side by side at the base. Here Y moves under
        // X; there X moves under Y. Each move alone is fine, together they
        // would hang X and Y from each other and off the tree.
        let b = tree(vec![
            branch(1, "X", vec![]),
            branch(2, "Y", vec![leaf(3, "z")]),
        ]);
        let l = tree(vec![branch(
            1,
            "X",
            vec![branch(2, "Y", vec![leaf(3, "z")])],
        )]);
        let r = tree(vec![branch(
            2,
            "Y",
            vec![leaf(3, "z"), branch(1, "X", vec![])],
        )]);
        for prefer in [Side::Local, Side::Remote] {
            let m = merge3(Some(&b), &l, &r, prefer);
            sound(&m);
            assert_eq!(m.tree.hashes().len(), 3, "{:?}", m.tree);
        }
    }
}
