use crate::language::Language;
use crate::matcher::Matcher;
use crate::replacer::Replacer;
use crate::ts_parser::{parse, perform_edit, Edit};

#[derive(Clone)]
pub struct Root<L: Language> {
    pub inner: tree_sitter::Tree,
    pub source: String,
    pub lang: L,
}

/// Represents [`tree_sitter::Tree`] and owns source string
/// Note: Root is not generic against [`Language`](crate::language::Language)
impl<L: Language> Root<L> {
    pub fn new(src: &str, lang: L) -> Self {
        Self {
            inner: parse(src, None, lang.get_ts_language()),
            source: src.into(),
            lang,
        }
    }
    // extract non generic implementation to reduce code size
    pub fn do_edit<'t>(&mut self, edit: Edit) {
        let input = unsafe { self.source.as_mut_vec() };
        let input_edit = perform_edit(&mut self.inner, input, &edit);
        self.inner.edit(&input_edit);
        self.inner = parse(&self.source, Some(&self.inner), self.lang.get_ts_language());
    }

    pub fn root(&self) -> Node<L> {
        Node {
            inner: self.inner.root_node(),
            root: self,
        }
    }
}

// the lifetime r represents root
#[derive(Clone, Copy)]
pub struct Node<'r, L: Language> {
    pub(crate) inner: tree_sitter::Node<'r>,
    pub(crate) root: &'r Root<L>,
}
pub type KindId = u16;

struct NodeWalker<'tree, L: Language> {
    cursor: tree_sitter::TreeCursor<'tree>,
    root: &'tree Root<L>,
    count: usize,
}

impl<'tree, L: Language> Iterator for NodeWalker<'tree, L> {
    type Item = Node<'tree, L>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        let ret = Some(Node {
            inner: self.cursor.node(),
            root: self.root,
        });
        self.cursor.goto_next_sibling();
        self.count -= 1;
        ret
    }
}

impl<'tree, L: Language> ExactSizeIterator for NodeWalker<'tree, L> {
    fn len(&self) -> usize {
        self.count
    }
}

pub struct DFS<'tree, L: Language> {
    cursor: tree_sitter::TreeCursor<'tree>,
    root: &'tree Root<L>,
    // record the starting node, if we return back to starting point
    // we should terminate the dfs.
    start_id: Option<usize>,
}

impl<'tree, L: Language> DFS<'tree, L> {
    fn new(node: &Node<'tree, L>) -> Self {
        Self {
            cursor: node.inner.walk(),
            root: node.root,
            start_id: Some(node.inner.id()),
        }
    }
}

impl<'tree, L: Language> Iterator for DFS<'tree, L> {
    type Item = Node<'tree, L>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start_id?;
        let cursor = &mut self.cursor;
        let inner = cursor.node();
        let ret = Some(Node {
            inner,
            root: self.root,
        });
        if cursor.goto_first_child() {
            return ret;
        }
        while cursor.node().id() != start {
            if cursor.goto_next_sibling() {
                return ret;
            }
            cursor.goto_parent();
        }
        self.start_id = None;
        ret
    }
}

// internal API
impl<'r, L: Language> Node<'r, L> {
    pub fn is_leaf(&self) -> bool {
        self.inner.child_count() == 0
    }
    pub fn kind(&self) -> &str {
        self.inner.kind()
    }
    pub fn kind_id(&self) -> KindId {
        self.inner.kind_id()
    }
    pub fn text(&self) -> &'r str {
        self.inner
            .utf8_text(self.root.source.as_bytes())
            .expect("invalid source text encoding")
    }

    pub fn children<'s>(&'s self) -> impl ExactSizeIterator<Item = Node<'r, L>> + 's {
        let mut cursor = self.inner.walk();
        cursor.goto_first_child();
        NodeWalker {
            cursor,
            root: self.root,
            count: self.inner.child_count(),
        }
    }

    pub fn dfs<'s>(&'s self) -> DFS<'r, L> {
        DFS::new(self)
    }

    pub fn display_context(&self) -> DisplayContext<'r> {
        let bytes = self.root.source.as_bytes();
        let start = self.inner.start_byte();
        let end = self.inner.end_byte();
        let (mut leading, mut trailing) = (start, end);
        while leading > 0 && bytes[leading - 1] != b'\n' {
            leading -= 1;
        }
        // tree-sitter will append line ending to source so trailing can be out of bound
        trailing = trailing.min(bytes.len() - 1);
        while trailing < bytes.len() - 1 && bytes[trailing + 1] != b'\n' {
            trailing += 1;
        }
        DisplayContext {
            matched: self.text(),
            leading: &self.root.source[leading..start],
            trailing: &self.root.source[end..=trailing],
            start_line: self.inner.start_position().row + 1,
        }
    }
}

pub struct DisplayContext<'r> {
    /// content for the matched node
    pub matched: &'r str,
    /// content before the matched node
    pub leading: &'r str,
    /// content after the matched node
    pub trailing: &'r str,
    /// start line of the matched node
    pub start_line: usize,
}

// tree traversal API
impl<'r, L: Language> Node<'r, L> {
    #[must_use]
    pub fn find<M: Matcher<L>>(&self, pat: M) -> Option<Self> {
        pat.find_node(*self)
    }

    pub fn find_all<M: Matcher<L>>(&self, pat: M) -> impl Iterator<Item = Node<'r, L>> {
        pat.find_all_nodes(*self)
    }

    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let inner = self.inner.parent()?;
        Some(Node {
            inner,
            root: self.root,
        })
    }

    #[must_use]
    pub fn nth_child(&self, nth: usize) -> Option<Self> {
        let inner = self.inner.child(nth)?;
        Some(Node {
            inner,
            root: self.root,
        })
    }

    pub fn ancestors(&self) -> impl Iterator<Item = Node<'r, L>> + '_ {
        let mut parent = self.inner.parent();
        std::iter::from_fn(move || {
            let inner = parent?;
            let ret = Some(Node {
                inner,
                root: self.root,
            });
            parent = inner.parent();
            ret
        })
    }
    #[must_use]
    pub fn next(&self) -> Option<Self> {
        let inner = self.inner.next_sibling()?;
        Some(Node {
            inner,
            root: self.root,
        })
    }
    pub fn next_all(&self) -> impl Iterator<Item = Node<'r, L>> + '_ {
        let mut cursor = self.inner.walk();
        let root = self.root;
        std::iter::from_fn(move || {
            if cursor.goto_next_sibling() {
                Some(Node {
                    inner: cursor.node(),
                    root,
                })
            } else {
                None
            }
        })
    }
    #[must_use]
    pub fn prev(&self) -> Option<Node<'r, L>> {
        let inner = self.inner.prev_sibling()?;
        Some(Node {
            inner,
            root: self.root,
        })
    }
    #[must_use]
    pub fn child(&self, i: usize) -> Option<Node<'r, L>> {
        let inner = self.inner.child(i)?;
        Some(Node {
            inner,
            root: self.root,
        })
    }
}

// r manipulation API
impl<'r, L: Language> Node<'r, L> {
    pub fn attr(&self) {}
    pub fn replace<M: Matcher<L>, R: Replacer<L>>(&self, matcher: M, replacer: R) -> Option<Edit> {
        let mut env = matcher.get_meta_var_env();
        let node = matcher.find_node_with_env(*self, &mut env)?;
        let inner = node.inner;
        let position = inner.start_byte();
        // instead of using start_byte/end_byte, ignore trivia like semicolon ;
        let named_cnt = inner.named_child_count();
        let end = inner.named_child(named_cnt - 1).unwrap().end_byte();
        let deleted_length = end - position;
        let inserted_text = replacer.generate_replacement(&env, self.root.lang);
        Some(Edit {
            position,
            deleted_length,
            inserted_text,
        })
    }
    pub fn replace_by(&self) {}
    pub fn after(&self) {}
    pub fn before(&self) {}
    pub fn append(&self) {}
    pub fn prepend(&self) {}
    pub fn empty(&self) {}
    pub fn remove(&self) {}
    pub fn clone(&self) {}
}

#[cfg(test)]
mod test {
    use crate::language::{Language, Tsx};
    #[test]
    fn test_is_leaf() {
        let root = Tsx.new("let a = 123");
        let node = root.root();
        assert!(!node.is_leaf());
    }

    #[test]
    fn test_children() {
        let root = Tsx.new("let a = 123");
        let node = root.root();
        let children: Vec<_> = node.children().collect();
        assert_eq!(children.len(), 1);
        let texts: Vec<_> = children[0]
            .children()
            .map(|c| c.text().to_string())
            .collect();
        assert_eq!(texts, vec!["let", "a = 123"]);
    }

    #[test]
    fn test_display_context() {
        // display context should not panic
        let s = "i()";
        assert_eq!(s.len(), 3);
        let root = Tsx.new(s);
        let node = root.root();
        assert_eq!(node.display_context().trailing.len(), 0);
    }
}
