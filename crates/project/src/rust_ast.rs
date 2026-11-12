//! Rust AST symbol and dependency extraction using syn.

use crate::models::{DependencyEdge, EdgeKind, SymbolKind, SymbolMeta};
use syn::spanned::Spanned;
use syn::{Item, Visibility};

pub struct RustAstExtractor;

impl RustAstExtractor {
    pub fn extract(
        file_path: &str,
        content: &str,
    ) -> Result<(Vec<SymbolMeta>, Vec<DependencyEdge>), syn::Error> {
        let syntax = syn::parse_file(content)?;
        let mut symbols = Vec::new();
        let mut edges = Vec::new();

        Self::visit_items(file_path, None, &syntax.items, &mut symbols, &mut edges);

        Ok((symbols, edges))
    }

    fn visit_items(
        file: &str,
        current_mod: Option<&str>,
        items: &[Item],
        symbols: &mut Vec<SymbolMeta>,
        edges: &mut Vec<DependencyEdge>,
    ) {
        for item in items {
            match item {
                Item::Fn(f) => {
                    let is_pub = matches!(f.vis, Visibility::Public(_));
                    let line = f.span().start().line;
                    let name = f.sig.ident.to_string();
                    let conditional = extract_cfg(&f.attrs);
                    symbols.push(SymbolMeta {
                        file: file.to_string(),
                        name,
                        kind: SymbolKind::Function,
                        is_public: is_pub,
                        line,
                        module_path: current_mod.map(|s| s.to_string()),
                        signature: Some(stringify_sig(&f.sig)),
                        conditional,
                    });
                }
                Item::Struct(s) => {
                    let is_pub = matches!(s.vis, Visibility::Public(_));
                    let line = s.span().start().line;
                    let name = s.ident.to_string();
                    let conditional = extract_cfg(&s.attrs);
                    symbols.push(SymbolMeta {
                        file: file.to_string(),
                        name,
                        kind: SymbolKind::Struct,
                        is_public: is_pub,
                        line,
                        module_path: current_mod.map(|s| s.to_string()),
                        signature: None,
                        conditional,
                    });
                }
                Item::Enum(e) => {
                    let is_pub = matches!(e.vis, Visibility::Public(_));
                    let line = e.span().start().line;
                    let name = e.ident.to_string();
                    let conditional = extract_cfg(&e.attrs);
                    symbols.push(SymbolMeta {
                        file: file.to_string(),
                        name,
                        kind: SymbolKind::Enum,
                        is_public: is_pub,
                        line,
                        module_path: current_mod.map(|s| s.to_string()),
                        signature: None,
                        conditional,
                    });
                }
                Item::Trait(t) => {
                    let is_pub = matches!(t.vis, Visibility::Public(_));
                    let line = t.span().start().line;
                    let name = t.ident.to_string();
                    let conditional = extract_cfg(&t.attrs);
                    symbols.push(SymbolMeta {
                        file: file.to_string(),
                        name,
                        kind: SymbolKind::Trait,
                        is_public: is_pub,
                        line,
                        module_path: current_mod.map(|s| s.to_string()),
                        signature: None,
                        conditional,
                    });
                }
                Item::Impl(im) => {
                    let line = im.span().start().line;
                    let target_name = stringify_type(&im.self_ty);
                    let trait_name = im.trait_.as_ref().map(|(_, path, _)| stringify_path(path));
                    let name = match &trait_name {
                        Some(tr) => format!("{tr} for {target_name}"),
                        None => target_name,
                    };
                    symbols.push(SymbolMeta {
                        file: file.to_string(),
                        name,
                        kind: SymbolKind::Impl,
                        is_public: true,
                        line,
                        module_path: current_mod.map(|s| s.to_string()),
                        signature: None,
                        conditional: extract_cfg(&im.attrs),
                    });

                    // Visit impl items (methods)
                    for impl_item in &im.items {
                        if let syn::ImplItem::Fn(method) = impl_item {
                            let is_pub = matches!(method.vis, Visibility::Public(_));
                            symbols.push(SymbolMeta {
                                file: file.to_string(),
                                name: method.sig.ident.to_string(),
                                kind: SymbolKind::Method,
                                is_public: is_pub,
                                line: method.span().start().line,
                                module_path: current_mod.map(|s| s.to_string()),
                                signature: Some(stringify_sig(&method.sig)),
                                conditional: extract_cfg(&method.attrs),
                            });
                        }
                    }
                }
                Item::Mod(m) => {
                    let is_pub = matches!(m.vis, Visibility::Public(_));
                    let mod_name = m.ident.to_string();
                    let full_mod = match current_mod {
                        Some(p) => format!("{p}::{mod_name}"),
                        None => mod_name.clone(),
                    };
                    symbols.push(SymbolMeta {
                        file: file.to_string(),
                        name: mod_name.clone(),
                        kind: SymbolKind::Module,
                        is_public: is_pub,
                        line: m.span().start().line,
                        module_path: current_mod.map(|s| s.to_string()),
                        signature: None,
                        conditional: extract_cfg(&m.attrs),
                    });

                    edges.push(DependencyEdge {
                        from_file: file.to_string(),
                        to_target: mod_name.clone(),
                        kind: EdgeKind::ModuleChild,
                        is_dynamic: false,
                        confidence: 1.0,
                    });

                    if let Some((_, nested_items)) = &m.content {
                        Self::visit_items(file, Some(&full_mod), nested_items, symbols, edges);
                    }
                }
                Item::Use(u) => {
                    let is_pub = matches!(u.vis, Visibility::Public(_));
                    let target = stringify_use_tree(&u.tree);
                    edges.push(DependencyEdge {
                        from_file: file.to_string(),
                        to_target: target.clone(),
                        kind: if is_pub { EdgeKind::ReExport } else { EdgeKind::Import },
                        is_dynamic: false,
                        confidence: 1.0,
                    });
                    if is_pub {
                        symbols.push(SymbolMeta {
                            file: file.to_string(),
                            name: target,
                            kind: SymbolKind::Export,
                            is_public: true,
                            line: u.span().start().line,
                            module_path: current_mod.map(|s| s.to_string()),
                            signature: None,
                            conditional: extract_cfg(&u.attrs),
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

fn extract_cfg(attrs: &[syn::Attribute]) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("cfg") {
            let meta = &attr.meta;
            if let syn::Meta::List(list) = meta {
                return Some(list.tokens.to_string());
            }
        }
    }
    None
}

fn stringify_sig(sig: &syn::Signature) -> String {
    let fn_name = &sig.ident;
    let inputs: Vec<String> = sig.inputs.iter().map(|arg| match arg {
        syn::FnArg::Receiver(_) => "self".to_string(),
        syn::FnArg::Typed(pat) => {
            let pat_str = match &*pat.pat {
                syn::Pat::Ident(id) => id.ident.to_string(),
                _ => "_".to_string(),
            };
            format!("{}: {}", pat_str, stringify_type(&pat.ty))
        }
    }).collect();
    let ret = match &sig.output {
        syn::ReturnType::Default => "".to_string(),
        syn::ReturnType::Type(_, ty) => format!(" -> {}", stringify_type(ty)),
    };
    format!("fn {}({}){}", fn_name, inputs.join(", "), ret)
}

fn stringify_type(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => stringify_path(&p.path),
        syn::Type::Reference(r) => {
            let mut s = "&".to_string();
            if r.mutability.is_some() {
                s.push_str("mut ");
            }
            s.push_str(&stringify_type(&r.elem));
            s
        }
        _ => "impl Type".to_string(),
    }
}

fn stringify_path(p: &syn::Path) -> String {
    p.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
}

fn stringify_use_tree(tree: &syn::UseTree) -> String {
    match tree {
        syn::UseTree::Path(p) => format!("{}::{}", p.ident, stringify_use_tree(&p.tree)),
        syn::UseTree::Name(n) => n.ident.to_string(),
        syn::UseTree::Rename(r) => format!("{} as {}", r.ident, r.rename),
        syn::UseTree::Glob(_) => "*".to_string(),
        syn::UseTree::Group(g) => {
            let items: Vec<String> = g.items.iter().map(stringify_use_tree).collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}
