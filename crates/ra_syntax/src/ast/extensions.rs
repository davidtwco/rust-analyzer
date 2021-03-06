//! Various extension methods to ast Nodes, which are hard to code-generate.
//! Extensions for various expressions live in a sibling `expr_extensions` module.

use itertools::Itertools;

use crate::{SmolStr, SyntaxToken, ast::{self, AstNode, children, child_opt}, SyntaxKind::*, SyntaxElement};
use ra_parser::SyntaxKind;

impl ast::Name {
    pub fn text(&self) -> &SmolStr {
        let ident = self.syntax().first_child_or_token().unwrap().as_token().unwrap();
        ident.text()
    }
}

impl ast::NameRef {
    pub fn text(&self) -> &SmolStr {
        let ident = self.syntax().first_child_or_token().unwrap().as_token().unwrap();
        ident.text()
    }
}

impl ast::Attr {
    pub fn is_inner(&self) -> bool {
        let tt = match self.value() {
            None => return false,
            Some(tt) => tt,
        };

        let prev = match tt.syntax().prev_sibling() {
            None => return false,
            Some(prev) => prev,
        };

        prev.kind() == EXCL
    }

    pub fn as_atom(&self) -> Option<SmolStr> {
        let tt = self.value()?;
        let (_bra, attr, _ket) = tt.syntax().children_with_tokens().collect_tuple()?;
        if attr.kind() == IDENT {
            Some(attr.as_token()?.text().clone())
        } else {
            None
        }
    }

    pub fn as_call(&self) -> Option<(SmolStr, &ast::TokenTree)> {
        let tt = self.value()?;
        let (_bra, attr, args, _ket) = tt.syntax().children_with_tokens().collect_tuple()?;
        let args = ast::TokenTree::cast(args.as_node()?)?;
        if attr.kind() == IDENT {
            Some((attr.as_token()?.text().clone(), args))
        } else {
            None
        }
    }

    pub fn as_named(&self) -> Option<SmolStr> {
        let tt = self.value()?;
        let attr = tt.syntax().children_with_tokens().nth(1)?;
        if attr.kind() == IDENT {
            Some(attr.as_token()?.text().clone())
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSegmentKind<'a> {
    Name(&'a ast::NameRef),
    SelfKw,
    SuperKw,
    CrateKw,
}

impl ast::PathSegment {
    pub fn parent_path(&self) -> &ast::Path {
        self.syntax()
            .parent()
            .and_then(ast::Path::cast)
            .expect("segments are always nested in paths")
    }

    pub fn kind(&self) -> Option<PathSegmentKind> {
        let res = if let Some(name_ref) = self.name_ref() {
            PathSegmentKind::Name(name_ref)
        } else {
            match self.syntax().first_child_or_token()?.kind() {
                SELF_KW => PathSegmentKind::SelfKw,
                SUPER_KW => PathSegmentKind::SuperKw,
                CRATE_KW => PathSegmentKind::CrateKw,
                _ => return None,
            }
        };
        Some(res)
    }

    pub fn has_colon_colon(&self) -> bool {
        match self.syntax.first_child_or_token().map(|s| s.kind()) {
            Some(COLONCOLON) => true,
            _ => false,
        }
    }
}

impl ast::Path {
    pub fn parent_path(&self) -> Option<&ast::Path> {
        self.syntax().parent().and_then(ast::Path::cast)
    }
}

impl ast::Module {
    pub fn has_semi(&self) -> bool {
        match self.syntax().last_child_or_token() {
            None => false,
            Some(node) => node.kind() == SEMI,
        }
    }
}

impl ast::UseTree {
    pub fn has_star(&self) -> bool {
        self.syntax().children_with_tokens().any(|it| it.kind() == STAR)
    }
}

impl ast::UseTreeList {
    pub fn parent_use_tree(&self) -> &ast::UseTree {
        self.syntax()
            .parent()
            .and_then(ast::UseTree::cast)
            .expect("UseTreeLists are always nested in UseTrees")
    }
}

impl ast::ImplBlock {
    pub fn target_type(&self) -> Option<&ast::TypeRef> {
        match self.target() {
            (Some(t), None) | (_, Some(t)) => Some(t),
            _ => None,
        }
    }

    pub fn target_trait(&self) -> Option<&ast::TypeRef> {
        match self.target() {
            (Some(t), Some(_)) => Some(t),
            _ => None,
        }
    }

    fn target(&self) -> (Option<&ast::TypeRef>, Option<&ast::TypeRef>) {
        let mut types = children(self);
        let first = types.next();
        let second = types.next();
        (first, second)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructKind<'a> {
    Tuple(&'a ast::PosFieldDefList),
    Named(&'a ast::NamedFieldDefList),
    Unit,
}

impl StructKind<'_> {
    fn from_node<N: AstNode>(node: &N) -> StructKind {
        if let Some(nfdl) = child_opt::<_, ast::NamedFieldDefList>(node) {
            StructKind::Named(nfdl)
        } else if let Some(pfl) = child_opt::<_, ast::PosFieldDefList>(node) {
            StructKind::Tuple(pfl)
        } else {
            StructKind::Unit
        }
    }
}

impl ast::StructDef {
    pub fn kind(&self) -> StructKind {
        StructKind::from_node(self)
    }
}

impl ast::EnumVariant {
    pub fn parent_enum(&self) -> &ast::EnumDef {
        self.syntax()
            .parent()
            .and_then(|it| it.parent())
            .and_then(ast::EnumDef::cast)
            .expect("EnumVariants are always nested in Enums")
    }
    pub fn kind(&self) -> StructKind {
        StructKind::from_node(self)
    }
}

impl ast::LetStmt {
    pub fn has_semi(&self) -> bool {
        match self.syntax().last_child_or_token() {
            None => false,
            Some(node) => node.kind() == SEMI,
        }
    }
}

impl ast::ExprStmt {
    pub fn has_semi(&self) -> bool {
        match self.syntax().last_child_or_token() {
            None => false,
            Some(node) => node.kind() == SEMI,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind<'a> {
    Name(&'a ast::NameRef),
    Index(SyntaxToken<'a>),
}

impl ast::FieldExpr {
    pub fn index_token(&self) -> Option<SyntaxToken> {
        self.syntax
            .children_with_tokens()
            // FIXME: Accepting floats here to reject them in validation later
            .find(|c| c.kind() == SyntaxKind::INT_NUMBER || c.kind() == SyntaxKind::FLOAT_NUMBER)
            .as_ref()
            .and_then(SyntaxElement::as_token)
    }

    pub fn field_access(&self) -> Option<FieldKind> {
        if let Some(nr) = self.name_ref() {
            Some(FieldKind::Name(nr))
        } else if let Some(tok) = self.index_token() {
            Some(FieldKind::Index(tok))
        } else {
            None
        }
    }
}

impl ast::RefPat {
    pub fn is_mut(&self) -> bool {
        self.syntax().children_with_tokens().any(|n| n.kind() == MUT_KW)
    }
}

impl ast::BindPat {
    pub fn is_mutable(&self) -> bool {
        self.syntax().children_with_tokens().any(|n| n.kind() == MUT_KW)
    }

    pub fn is_ref(&self) -> bool {
        self.syntax().children_with_tokens().any(|n| n.kind() == REF_KW)
    }
}

impl ast::PointerType {
    pub fn is_mut(&self) -> bool {
        self.syntax().children_with_tokens().any(|n| n.kind() == MUT_KW)
    }
}

impl ast::ReferenceType {
    pub fn is_mut(&self) -> bool {
        self.syntax().children_with_tokens().any(|n| n.kind() == MUT_KW)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SelfParamKind {
    /// self
    Owned,
    /// &self
    Ref,
    /// &mut self
    MutRef,
}

impl ast::SelfParam {
    pub fn self_kw_token(&self) -> SyntaxToken {
        self.syntax()
            .children_with_tokens()
            .filter_map(|it| it.as_token())
            .find(|it| it.kind() == SELF_KW)
            .expect("invalid tree: self param must have self")
    }

    pub fn kind(&self) -> SelfParamKind {
        let borrowed = self.syntax().children_with_tokens().any(|n| n.kind() == AMP);
        if borrowed {
            // check for a `mut` coming after the & -- `mut &self` != `&mut self`
            if self
                .syntax()
                .children_with_tokens()
                .skip_while(|n| n.kind() != AMP)
                .any(|n| n.kind() == MUT_KW)
            {
                SelfParamKind::MutRef
            } else {
                SelfParamKind::Ref
            }
        } else {
            SelfParamKind::Owned
        }
    }
}

impl ast::LifetimeParam {
    pub fn lifetime_token(&self) -> Option<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|it| it.as_token())
            .find(|it| it.kind() == LIFETIME)
    }
}

impl ast::WherePred {
    pub fn lifetime_token(&self) -> Option<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|it| it.as_token())
            .find(|it| it.kind() == LIFETIME)
    }
}
