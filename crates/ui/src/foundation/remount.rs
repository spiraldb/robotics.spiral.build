//! Remounting a subtree when the thing it shows is another thing.

use dioxus::prelude::*;

/// Children that mount afresh whenever `identity` changes, dropping every hook,
/// resource, and task the previous identity's scopes held.
///
/// Dioxus reads `key` only among the items of a list. On a lone component it
/// is ignored: a changed prop re-renders the same scope, whose hooks still
/// hold what they captured at mount — a recording's clock and streams, a
/// node's search, a dataset's episode. A list of one keyed by the identity is
/// the one form the differ replaces rather than updates, so that is what this
/// renders.
#[component]
pub fn Remount(identity: String, children: Element) -> Element {
    rsx! {
        for identity in std::iter::once(identity) {
            Fragment { key: "{identity}", {children.clone()} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the probe saw: the identity each mount captured.
    #[derive(Clone, Default)]
    struct Mounts(Rc<RefCell<Vec<String>>>);

    impl PartialEq for Mounts {
        fn eq(&self, other: &Self) -> bool {
            Rc::ptr_eq(&self.0, &other.0)
        }
    }

    #[component]
    fn Probe(identity: String, mounts: Mounts) -> Element {
        // Captured once per mount, the way a resource captures its episode.
        use_hook(|| mounts.0.borrow_mut().push(identity.clone()));
        rsx! {}
    }

    #[derive(Clone, Default)]
    struct Harness {
        mounts: Mounts,
        identity: Rc<RefCell<Option<Signal<String>>>>,
    }

    impl Harness {
        fn render(self) -> Element {
            let identity = use_signal(|| "first".to_owned());
            *self.identity.borrow_mut() = Some(identity);
            let mounts = self.mounts.clone();
            rsx! {
                Remount { identity: identity(),
                    Probe { identity: identity(), mounts }
                }
            }
        }
    }

    #[test]
    fn another_identity_mounts_its_children_again_and_the_same_one_does_not() {
        let harness = Harness::default();
        let mut dom = VirtualDom::new_with_props(Harness::render, harness.clone());
        dom.rebuild_in_place();
        let mut identity = harness.identity.borrow().expect("the harness rendered");

        dom.in_scope(ScopeId::ROOT, || identity.set("second".to_owned()));
        dom.render_immediate_to_vec();
        dom.in_scope(ScopeId::ROOT, || identity.set("second".to_owned()));
        dom.render_immediate_to_vec();

        assert_eq!(*harness.mounts.0.borrow(), ["first", "second"]);
    }
}
