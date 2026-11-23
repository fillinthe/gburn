//! The menus as data: one description (built in `App::menu_model`) drives the in-window bar and, on a Mac, the
//! system menu bar at the top of the screen.

#[derive(Clone, PartialEq, Debug)]
pub enum Node {
    Item { label: String, key: String, on: bool, id: String },
    Sep,
    Sub { label: String, on: bool, items: Vec<Node> },
}

pub type Top = (String, Vec<Node>);

/// What the menu code talks to: `item`, `sep`, `sub` (a submenu), `enabled` (greys everything inside) and `top`.
#[derive(Default)]
pub struct Builder {
    stack: Vec<Vec<Node>>,
    off: usize,
    pub tops: Vec<Top>,
}

impl Builder {
    pub fn top(&mut self, label: &str, f: impl FnOnce(&mut Builder)) {
        self.stack.push(Vec::new());
        f(self);
        let items = self.stack.pop().unwrap_or_default();
        self.tops.push((label.to_string(), items));
    }
    pub fn item(&mut self, label: &str, key: &str, on: bool, id: &str) {
        let on = on && self.off == 0;
        if let Some(v) = self.stack.last_mut() {
            v.push(Node::Item { label: label.into(), key: key.into(), on, id: id.into() });
        }
    }
    pub fn sep(&mut self) {
        if let Some(v) = self.stack.last_mut() {
            v.push(Node::Sep);
        }
    }
    pub fn sub(&mut self, label: &str, f: impl FnOnce(&mut Builder)) {
        let on = self.off == 0;
        self.stack.push(Vec::new());
        f(self);
        let items = self.stack.pop().unwrap_or_default();
        if let Some(v) = self.stack.last_mut() {
            v.push(Node::Sub { label: label.into(), on, items });
        }
    }
    pub fn enabled(&mut self, on: bool, f: impl FnOnce(&mut Builder)) {
        self.off += usize::from(!on);
        f(self);
        self.off -= usize::from(!on);
    }
}
