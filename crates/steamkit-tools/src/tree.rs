//! A labelled tree, printed like the NetHook analyzer's tree view.

use std::fmt;

/// A node with a label and children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    /// The text shown for the node.
    pub label: String,
    /// The child nodes.
    pub children: Vec<Node>,
}

impl Node {
    /// A node without children.
    pub fn leaf(label: impl Into<String>) -> Self {
        Self { label: label.into(), children: Vec::new() }
    }

    /// A node with children.
    pub fn branch(label: impl Into<String>, children: Vec<Node>) -> Self {
        Self { label: label.into(), children }
    }

    /// The first descendant (depth first) whose label starts with `prefix`.
    pub fn find(&self, prefix: &str) -> Option<&Node> {
        if self.label.starts_with(prefix) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(prefix))
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, prefix: &str) -> fmt::Result {
        for (i, child) in self.children.iter().enumerate() {
            let last = i + 1 == self.children.len();
            let (branch, indent) = if last { ("└─ ", "   ") } else { ("├─ ", "│  ") };
            writeln!(f, "{prefix}{branch}{}", child.label)?;
            child.write(f, &format!("{prefix}{indent}"))?;
        }
        Ok(())
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.label)?;
        self.write(f, "")
    }
}

/// A short hex preview of bytes.
pub fn hex_preview(data: &[u8]) -> String {
    const MAX: usize = 32;
    let mut s: String = data.iter().take(MAX).map(|b| format!("{b:02x}")).collect();
    if data.len() > MAX {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders() {
        let tree =
            Node::branch("root", vec![Node::branch("a", vec![Node::leaf("a1"), Node::leaf("a2")]), Node::leaf("b")]);
        assert_eq!(tree.to_string(), "root\n├─ a\n│  ├─ a1\n│  └─ a2\n└─ b\n");
        assert_eq!(tree.find("a2").unwrap().label, "a2");
        assert_eq!(hex_preview(&[1, 0xab]), "01ab");
    }
}
