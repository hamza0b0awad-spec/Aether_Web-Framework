//! # Radix Trie Node Representation

use aether_http::method::Method;

pub type HandlerId = u32;

#[derive(Clone, Debug)]
pub enum NodeType {
    Static,
    Param(String),
    CatchAll(String),
}

#[derive(Clone, Debug)]
pub struct RouteNode {
    pub prefix: String,
    pub node_type: NodeType,
    pub allowed_methods: u64,
    pub handlers: Vec<(Method, HandlerId)>,
    /// The dynamic names of the route `method` registered at this node, in path order. A radix
    /// level is shared by every route that competes for the same request segment, and a route's
    /// handler lives on its own leaf — `/users/{id}` on the `Param` node, `/users/{user}/edit` on
    /// its `edit` child — so the names a capture gets are read from the node that answered, not
    /// from the node that happened to be created first.
    pub param_names: Vec<(Method, Vec<String>)>,
    pub children: Vec<RouteNode>,
}

impl RouteNode {
    pub fn new(prefix: String, node_type: NodeType) -> Self {
        Self {
            prefix,
            node_type,
            allowed_methods: 0,
            handlers: Vec::new(),
            param_names: Vec::new(),
            children: Vec::new(),
        }
    }

    #[inline(always)]
    pub fn supports_method(&self, method: Method) -> bool {
        (self.allowed_methods & method.bitmask()) != 0
    }

    pub fn get_handler(&self, method: Method) -> Option<HandlerId> {
        if !self.supports_method(method) {
            return None;
        }
        self.handlers
            .iter()
            .find(|(m, _)| *m == method)
            .map(|(_, h)| *h)
    }

    pub fn add_handler(&mut self, method: Method, handler: HandlerId) {
        self.allowed_methods |= method.bitmask();
        self.handlers.push((method, handler));
    }

    /// Record the ordered dynamic names of the route `method` registered at this node.
    /// `reject_duplicate` guarantees at most one route per method per node, so this only appends.
    pub fn set_param_names(&mut self, method: Method, names: Vec<String>) {
        self.param_names.push((method, names));
    }

    /// The names `method`'s route on this node declared, in path order — the same order the
    /// matcher pushes captures while descending toward this node.
    pub fn param_names_for(&self, method: Method) -> Option<&[String]> {
        self.param_names
            .iter()
            .find(|(m, _)| *m == method)
            .map(|(_, n)| n.as_slice())
    }
}
