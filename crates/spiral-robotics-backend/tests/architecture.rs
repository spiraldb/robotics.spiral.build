use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use syn::visit::{self, Visit};
use syn::{
    Attribute, BinOp, Expr, ExprBinary, ExprField, Fields, FnArg, ImplItem, Item, ItemEnum,
    ItemStruct, Member, Pat, PatStruct, ReturnType, Signature, TraitItem, Type, Visibility,
};

fn collect_production_rust_sources(directory: &Path, sources: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("source directory must exist") {
        let entry = entry.expect("source entry must be readable");
        let path = entry.path();
        if entry
            .file_type()
            .expect("source type must be readable")
            .is_dir()
        {
            if path.file_name().and_then(|value| value.to_str()) != Some("tests") {
                collect_production_rust_sources(&path, sources);
            }
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs")
            && path.file_name().and_then(|value| value.to_str()) != Some("tests.rs")
        {
            sources.push(path);
        }
    }
}

fn declares_local_dependency(manifest: &str, dependency: &str) -> bool {
    let package = manifest
        .split_once("[package]")
        .map_or(manifest, |(_, package)| package);
    let workspace_key = format!("{dependency}.workspace");
    let table_key = format!("{dependency} =");
    package.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with(&workspace_key) || line.starts_with(&table_key)
    })
}

fn has_cfg_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        if !attribute.path().is_ident("cfg") {
            return false;
        }
        let mut is_test = false;
        let _ = attribute.parse_nested_meta(|meta| {
            is_test |= meta.path.is_ident("test");
            Ok(())
        });
        is_test
    })
}

struct ComplexityBoundaryVisitor<'a> {
    source_path: &'a Path,
    violations: Vec<String>,
}

impl ComplexityBoundaryVisitor<'_> {
    fn check_signature(&mut self, signature: &Signature) {
        let parameter_count = signature.inputs.len();
        let required_external_source_method =
            signature.ident == "next_morsel" && self.source_path.ends_with("spiral_database.rs");
        if parameter_count > 3 && !required_external_source_method {
            self.violations.push(format!(
                "{} function {:?} has {parameter_count} parameters",
                self.source_path.display(),
                signature.ident,
            ));
        }
    }

    fn check_fields(&mut self, item_name: &str, fields: &Fields) {
        let field_count = fields.len();
        if field_count > 7 {
            self.violations.push(format!(
                "{} type {item_name:?} has {field_count} fields",
                self.source_path.display(),
            ));
        }
        for field in fields {
            if !matches!(field.vis, Visibility::Inherited) {
                let field_name = field
                    .ident
                    .as_ref()
                    .map_or_else(|| "<tuple field>".to_owned(), ToString::to_string);
                self.violations.push(format!(
                    "{} type {item_name:?} exposes field {field_name:?}",
                    self.source_path.display(),
                ));
            }
        }
    }

    fn check_free_function_ownership(&mut self, signature: &Signature) {
        let name = signature.ident.to_string();
        let mut ownership_violation = false;
        if [
            "build_",
            "capability_",
            "contract_",
            "create_",
            "decode_",
            "default_",
            "encode_",
            "ensure_",
            "format_",
            "humanize_",
            "load_",
            "normalize_",
            "parse_",
            "reduce_",
            "run_",
            "short_",
            "toggle_",
            "validate_",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
            || name == "discover"
        {
            ownership_violation = true;
            self.violations.push(format!(
                "{} free function {name:?} names type-owned behavior; use a receiver/associated method or From/TryFrom",
                self.source_path.display(),
            ));
        }

        if signature.inputs.iter().any(|argument| match argument {
            FnArg::Receiver(_) => false,
            FnArg::Typed(argument) => type_mentions(&argument.ty, "AppState"),
        }) {
            ownership_violation = true;
            self.violations.push(format!(
                "{} free function {name:?} orchestrates AppState; make it an AppState method",
                self.source_path.display(),
            ));
        }

        if let ReturnType::Type(_, output) = &signature.output
            && direct_type_name(output).is_some_and(|name| name.ends_with("Error"))
        {
            ownership_violation = true;
            self.violations.push(format!(
                "{} free function {name:?} constructs an error; put the constructor on the error type",
                self.source_path.display(),
            ));
        }

        let frontend_source = self
            .source_path
            .components()
            .any(|component| component.as_os_str().to_str() == Some("spiral-robotics-frontend"));
        let dioxus_element = matches!(
            &signature.output,
            ReturnType::Type(_, output)
                if direct_type_name(output).as_deref() == Some("Element")
        );
        let dioxus_context_hook = name.starts_with("use_")
            || [
                "data_controller",
                "description",
                "rollout_controller",
                "studio",
                "timeline_controller",
                "training_controller",
                "workflow_controller",
            ]
            .contains(&name.as_str());
        let framework_boundary = frontend_source && (dioxus_element || dioxus_context_hook);
        if !framework_boundary && name != "main" && !ownership_violation {
            self.violations.push(format!(
                "{} free production function {name:?} has no owner; use a receiver/associated method, From/TryFrom, or a local extension trait",
                self.source_path.display(),
            ));
        }
    }
}

fn type_mentions(r#type: &Type, expected: &str) -> bool {
    struct TypeNameVisitor<'a> {
        expected: &'a str,
        found: bool,
    }

    impl<'ast> Visit<'ast> for TypeNameVisitor<'_> {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            self.found |= path
                .segments
                .iter()
                .any(|segment| segment.ident == self.expected);
            if !self.found {
                visit::visit_path(self, path);
            }
        }
    }

    let mut visitor = TypeNameVisitor {
        expected,
        found: false,
    };
    visitor.visit_type(r#type);
    visitor.found
}

fn direct_type_name(r#type: &Type) -> Option<String> {
    match r#type {
        Type::Group(group) => direct_type_name(&group.elem),
        Type::Paren(paren) => direct_type_name(&paren.elem),
        Type::Reference(reference) => direct_type_name(&reference.elem),
        Type::Path(path) if path.qself.is_none() => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

impl<'ast> Visit<'ast> for ComplexityBoundaryVisitor<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        let attributes: &[Attribute] = match item {
            Item::Const(item) => &item.attrs,
            Item::Enum(item) => &item.attrs,
            Item::ExternCrate(item) => &item.attrs,
            Item::Fn(item) => &item.attrs,
            Item::ForeignMod(item) => &item.attrs,
            Item::Impl(item) => &item.attrs,
            Item::Macro(item) => &item.attrs,
            Item::Mod(item) => &item.attrs,
            Item::Static(item) => &item.attrs,
            Item::Struct(item) => &item.attrs,
            Item::Trait(item) => &item.attrs,
            Item::TraitAlias(item) => &item.attrs,
            Item::Type(item) => &item.attrs,
            Item::Union(item) => &item.attrs,
            Item::Use(item) => &item.attrs,
            _ => &[],
        };
        if !has_cfg_test(attributes) {
            visit::visit_item(self, item);
        }
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        let attributes: &[Attribute] = match item {
            ImplItem::Const(item) => &item.attrs,
            ImplItem::Fn(item) => &item.attrs,
            ImplItem::Type(item) => &item.attrs,
            ImplItem::Macro(item) => &item.attrs,
            _ => &[],
        };
        if !has_cfg_test(attributes) {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        let attributes: &[Attribute] = match item {
            TraitItem::Const(item) => &item.attrs,
            TraitItem::Fn(item) => &item.attrs,
            TraitItem::Type(item) => &item.attrs,
            TraitItem::Macro(item) => &item.attrs,
            _ => &[],
        };
        if !has_cfg_test(attributes) {
            visit::visit_trait_item(self, item);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.check_signature(&function.sig);
        self.check_free_function_ownership(&function.sig);
        visit::visit_item_fn(self, function);
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        self.check_signature(&function.sig);
        visit::visit_impl_item_fn(self, function);
    }

    fn visit_trait_item_fn(&mut self, function: &'ast syn::TraitItemFn) {
        self.check_signature(&function.sig);
        visit::visit_trait_item_fn(self, function);
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        self.check_fields(&item.ident.to_string(), &item.fields);
        visit::visit_item_struct(self, item);
    }

    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        for variant in &item.variants {
            self.check_fields(
                &format!("{}::{}", item.ident, variant.ident),
                &variant.fields,
            );
        }
        visit::visit_item_enum(self, item);
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        self.violations.push(format!(
            "{} declares static {:?}",
            self.source_path.display(),
            item.ident,
        ));
        visit::visit_item_static(self, item);
    }
}

struct PrivateFieldDeclaration {
    source_path: String,
    owner: String,
    equality_owner: String,
    name: String,
    derives_equality: bool,
}

#[derive(Default)]
struct PrivateFieldUsageVisitor {
    current_source_path: String,
    declarations: Vec<PrivateFieldDeclaration>,
    explicit_uses: BTreeSet<String>,
    equality_consumers: BTreeSet<String>,
    binding_types: Vec<BTreeMap<String, String>>,
    production_macro_tokens: String,
}

impl PrivateFieldUsageVisitor {
    fn begin_source(&mut self, source_path: &Path) {
        self.current_source_path = source_path.display().to_string();
    }

    fn record_named_fields(
        &mut self,
        owner: String,
        equality_owner: String,
        fields: &Fields,
        derives_equality: bool,
    ) {
        let Fields::Named(fields) = fields else {
            return;
        };
        self.declarations.extend(
            fields
                .named
                .iter()
                .filter(|field| matches!(field.vis, Visibility::Inherited))
                .filter_map(|field| field.ident.as_ref())
                .map(ToString::to_string)
                .filter(|name| !name.starts_with('_'))
                .map(|name| PrivateFieldDeclaration {
                    source_path: self.current_source_path.clone(),
                    owner: owner.clone(),
                    equality_owner: equality_owner.clone(),
                    name,
                    derives_equality,
                }),
        );
    }

    fn derives_partial_eq(attributes: &[Attribute]) -> bool {
        attributes.iter().any(|attribute| {
            if !attribute.path().is_ident("derive") {
                return false;
            }
            let mut derives_partial_eq = false;
            let _ = attribute.parse_nested_meta(|meta| {
                derives_partial_eq |= meta
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "PartialEq");
                Ok(())
            });
            derives_partial_eq
        })
    }

    fn push_signature_bindings(&mut self, signature: &Signature) {
        let bindings = signature
            .inputs
            .iter()
            .filter_map(|argument| {
                let FnArg::Typed(argument) = argument else {
                    return None;
                };
                let Pat::Ident(pattern) = argument.pat.as_ref() else {
                    return None;
                };
                direct_type_name(&argument.ty).map(|name| (pattern.ident.to_string(), name))
            })
            .collect();
        self.binding_types.push(bindings);
    }

    fn binding_type(&self, name: &str) -> Option<String> {
        self.binding_types
            .iter()
            .rev()
            .find_map(|bindings| bindings.get(name).cloned())
    }

    fn expression_type(&self, expression: &Expr) -> Option<String> {
        match expression {
            Expr::Group(group) => self.expression_type(&group.expr),
            Expr::Paren(paren) => self.expression_type(&paren.expr),
            Expr::Reference(reference) => self.expression_type(&reference.expr),
            Expr::Unary(unary) => self.expression_type(&unary.expr),
            Expr::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => {
                self.binding_type(&path.path.segments.first()?.ident.to_string())
            }
            _ => None,
        }
    }

    fn derive_supplies_semantics(attributes: &[Attribute]) -> bool {
        attributes.iter().any(|attribute| {
            if !attribute.path().is_ident("derive") {
                return false;
            }
            let mut supplies_semantics = false;
            let _ = attribute.parse_nested_meta(|meta| {
                supplies_semantics |= meta.path.segments.last().is_some_and(|segment| {
                    matches!(
                        segment.ident.to_string().as_str(),
                        "Args"
                            | "Deserialize"
                            | "Error"
                            | "Hash"
                            | "Ord"
                            | "Parser"
                            | "PartialOrd"
                            | "Pod"
                            | "Props"
                            | "Serialize"
                            | "Subcommand"
                            | "ValueEnum"
                            | "Zeroable"
                    )
                });
                Ok(())
            });
            supplies_semantics
        })
    }

    fn has_layout_contract(attributes: &[Attribute]) -> bool {
        attributes
            .iter()
            .any(|attribute| attribute.path().is_ident("repr"))
    }

    fn unconsumed_fields(self) -> Vec<String> {
        self.declarations
            .into_iter()
            .filter(|field| {
                !self.explicit_uses.contains(&field.name)
                    && !(field.derives_equality
                        && self.equality_consumers.contains(&field.equality_owner))
                    && !self
                        .production_macro_tokens
                        .contains(&format!(".{}", field.name))
                    && !self
                        .production_macro_tokens
                        .contains(&format!(". {}", field.name))
            })
            .map(|field| {
                format!(
                    "{} private field {}::{} is only initialized or passively derived",
                    field.source_path, field.owner, field.name
                )
            })
            .collect()
    }
}

impl<'ast> Visit<'ast> for PrivateFieldUsageVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        let attributes: &[Attribute] = match item {
            Item::Const(item) => &item.attrs,
            Item::Enum(item) => &item.attrs,
            Item::ExternCrate(item) => &item.attrs,
            Item::Fn(item) => &item.attrs,
            Item::ForeignMod(item) => &item.attrs,
            Item::Impl(item) => &item.attrs,
            Item::Macro(item) => &item.attrs,
            Item::Mod(item) => &item.attrs,
            Item::Static(item) => &item.attrs,
            Item::Struct(item) => &item.attrs,
            Item::Trait(item) => &item.attrs,
            Item::TraitAlias(item) => &item.attrs,
            Item::Type(item) => &item.attrs,
            Item::Union(item) => &item.attrs,
            Item::Use(item) => &item.attrs,
            _ => &[],
        };
        if !has_cfg_test(attributes) {
            visit::visit_item(self, item);
        }
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        let attributes: &[Attribute] = match item {
            ImplItem::Const(item) => &item.attrs,
            ImplItem::Fn(item) => &item.attrs,
            ImplItem::Type(item) => &item.attrs,
            ImplItem::Macro(item) => &item.attrs,
            _ => &[],
        };
        if !has_cfg_test(attributes) {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        if !Self::derive_supplies_semantics(&item.attrs) && !Self::has_layout_contract(&item.attrs)
        {
            let owner = item.ident.to_string();
            self.record_named_fields(
                owner.clone(),
                owner,
                &item.fields,
                Self::derives_partial_eq(&item.attrs),
            );
        }
        visit::visit_item_struct(self, item);
    }

    fn visit_item_enum(&mut self, item: &'ast ItemEnum) {
        if !Self::derive_supplies_semantics(&item.attrs) && !Self::has_layout_contract(&item.attrs)
        {
            let equality_owner = item.ident.to_string();
            for variant in &item.variants {
                self.record_named_fields(
                    format!("{}::{}", item.ident, variant.ident),
                    equality_owner.clone(),
                    &variant.fields,
                    Self::derives_partial_eq(&item.attrs),
                );
            }
        }
        visit::visit_item_enum(self, item);
    }

    fn visit_expr_field(&mut self, expression: &'ast ExprField) {
        if let Member::Named(member) = &expression.member {
            self.explicit_uses.insert(member.to_string());
        }
        visit::visit_expr_field(self, expression);
    }

    fn visit_expr_binary(&mut self, expression: &'ast ExprBinary) {
        if matches!(expression.op, BinOp::Eq(_) | BinOp::Ne(_))
            && let Some(owner) = self
                .expression_type(&expression.left)
                .or_else(|| self.expression_type(&expression.right))
        {
            self.equality_consumers.insert(owner);
        }
        visit::visit_expr_binary(self, expression);
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.push_signature_bindings(&function.sig);
        visit::visit_item_fn(self, function);
        self.binding_types.pop();
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        self.push_signature_bindings(&function.sig);
        visit::visit_impl_item_fn(self, function);
        self.binding_types.pop();
    }

    fn visit_pat_struct(&mut self, pattern: &'ast PatStruct) {
        self.explicit_uses.extend(
            pattern
                .fields
                .iter()
                .filter_map(|field| match &field.member {
                    Member::Named(member) => Some(member.to_string()),
                    Member::Unnamed(_) => None,
                }),
        );
        visit::visit_pat_struct(self, pattern);
    }

    fn visit_macro(&mut self, r#macro: &'ast syn::Macro) {
        self.production_macro_tokens
            .push_str(&r#macro.tokens.to_string());
        visit::visit_macro(self, r#macro);
    }
}

#[test]
fn first_party_items_stay_within_cognitive_complexity_bounds() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("backend crate lives under workspace/crates");
    let crates_root = workspace_root.join("crates");
    let mut sources = Vec::new();
    for entry in fs::read_dir(&crates_root).expect("workspace crates directory must exist") {
        let entry = entry.expect("workspace crate entry must be readable");
        if !entry
            .file_type()
            .expect("crate entry type must be readable")
            .is_dir()
        {
            continue;
        }
        let crate_root = entry.path();
        if crate_root.join("Cargo.toml").is_file() && crate_root.join("src").is_dir() {
            collect_production_rust_sources(&crate_root, &mut sources);
        }
    }
    sources.sort();
    let mut violations = Vec::new();
    for source_path in sources {
        let source = fs::read_to_string(&source_path).expect("Rust source must be readable");
        let syntax = syn::parse_file(&source)
            .unwrap_or_else(|error| panic!("{} must parse: {error}", source_path.display()));
        let mut visitor = ComplexityBoundaryVisitor {
            source_path: &source_path,
            violations: Vec::new(),
        };
        visitor.visit_file(&syntax);
        violations.extend(visitor.violations);
    }
    assert!(
        violations.is_empty(),
        "first-party architecture bounds were exceeded:\n{}",
        violations.join("\n")
    );
}

#[test]
fn private_fields_have_explicit_production_consumers_or_external_semantics() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("backend crate lives under workspace/crates");
    let mut violations = Vec::new();
    for entry in
        fs::read_dir(workspace_root.join("crates")).expect("workspace crates directory must exist")
    {
        let entry = entry.expect("workspace crate entry must be readable");
        let crate_root = entry.path();
        if !entry
            .file_type()
            .expect("crate entry type must be readable")
            .is_dir()
            || !crate_root.join("Cargo.toml").is_file()
        {
            continue;
        }
        let mut sources = Vec::new();
        collect_production_rust_sources(&crate_root, &mut sources);
        sources.sort();
        let mut visitor = PrivateFieldUsageVisitor::default();
        for source_path in sources {
            let source = fs::read_to_string(&source_path).expect("Rust source must be readable");
            let syntax = syn::parse_file(&source)
                .unwrap_or_else(|error| panic!("{} must parse: {error}", source_path.display()));
            visitor.begin_source(&source_path);
            visitor.visit_file(&syntax);
        }
        violations.extend(visitor.unconsumed_fields());
    }

    assert!(
        violations.is_empty(),
        "private fields must have explicit production consumers or derive a reviewed external/key/layout contract:\n{}",
        violations.join("\n")
    );
}

#[test]
fn derived_equality_counts_only_when_production_compares_the_type() {
    let syntax = syn::parse_file(
        r#"
        #[derive(PartialEq)]
        struct PassiveEquality {
            derive_only_identity: u64,
        }

        #[derive(PartialEq)]
        struct ComparedIdentity {
            compared_identity: u64,
        }

        impl ComparedIdentity {
            fn matches(&self, other: ComparedIdentity) -> bool {
                *self == other
            }
        }
        "#,
    )
    .expect("equality audit fixture must parse");
    let mut visitor = PrivateFieldUsageVisitor::default();
    visitor.begin_source(Path::new("equality-audit-fixture.rs"));
    visitor.visit_file(&syntax);

    let violations = visitor.unconsumed_fields();
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("PassiveEquality::derive_only_identity"));
}

#[test]
fn product_crates_follow_functional_boundaries() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = backend.parent().expect("workspace crates directory");
    for present in [
        "spiral-robotics-api",
        "spiral-robotics-backend",
        "spiral-robotics-frontend",
        "spiral-robotics-hardware",
        "spiral-robotics-storage",
        "spiral-robotics-studio",
    ] {
        assert!(
            crates.join(present).join("Cargo.toml").is_file(),
            "functional crate {present:?} must exist"
        );
    }
    for retired in [
        "spiral-robotics-arm",
        "spiral-robotics-boundary-build",
        "spiral-robotics-contracts",
        "spiral-robotics-core",
        "spiral-robotics-sql",
        "spiral-robotics-studio-application",
        "spiral-robotics-studio-backend",
        "spiral-robotics-training-http",
        "spiral-robotics-ui",
    ] {
        assert!(
            !crates.join(retired).exists(),
            "engineering-convenience crate {retired:?} must stay removed"
        );
    }
    let workspace = crates
        .parent()
        .expect("crates directory lives in the workspace");
    assert!(
        !workspace.join("contracts").exists(),
        "generic root contracts directory must stay split between functional owners"
    );
    for metadata in [
        "spiral-robotics-storage/schema/version.json",
        "spiral-robotics-training/protocol/version.json",
    ] {
        assert!(
            crates.join(metadata).is_file(),
            "owned compatibility metadata {metadata:?} must exist"
        );
    }

    let backend_library = fs::read_to_string(backend.join("src/lib.rs")).expect("backend library");
    for capability in [
        "data",
        "recording",
        "robots",
        "rollout",
        "training",
        "workflow",
    ] {
        assert!(
            backend_library.contains(&format!("mod {capability};"))
                && !backend_library.contains(&format!("pub mod {capability};")),
            "backend capability {capability:?} must stay internal to the launcher crate"
        );
    }
    assert!(
        backend_library.contains("pub use http::server::StudioServer;"),
        "the backend's sole external capability must remain its launcher"
    );
}

#[test]
fn workspace_public_surface_stays_explicit_and_state_has_owners() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = backend.parent().expect("workspace crates directory");

    let backend_library = fs::read_to_string(backend.join("src/lib.rs")).expect("backend library");
    let backend_exports = backend_library
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("pub "))
        .collect::<Vec<_>>();
    assert_eq!(backend_exports, ["pub use http::server::StudioServer;"]);

    let frontend_library = fs::read_to_string(crates.join("spiral-robotics-frontend/src/lib.rs"))
        .expect("frontend library");
    let frontend_exports = frontend_library
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("pub "))
        .collect::<Vec<_>>();
    assert_eq!(frontend_exports, ["pub use shell::App;"]);

    let storage_library = fs::read_to_string(crates.join("spiral-robotics-storage/src/lib.rs"))
        .expect("storage library");
    for implementation_module in [
        "database",
        "manager",
        "project_store",
        "schema",
        "settings",
        "spiral_database",
        "sql",
    ] {
        assert!(
            !storage_library.contains(&format!("pub mod {implementation_module};")),
            "storage implementation module {implementation_module:?} must not become public"
        );
    }

    for entry in fs::read_dir(crates).expect("workspace crates must be readable") {
        let entry = entry.expect("workspace crate entry must be readable");
        if !entry.file_type().expect("crate entry type").is_dir() {
            continue;
        }
        let source_root = entry.path().join("src");
        if !source_root.is_dir() {
            continue;
        }
        let mut sources = Vec::new();
        collect_production_rust_sources(&source_root, &mut sources);
        for path in sources {
            let source = fs::read_to_string(&path).expect("Rust source must be readable");
            for (line_index, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default().trim();
                let tokens = code
                    .split(|character: char| {
                        !(character == '_' || character.is_ascii_alphanumeric())
                    })
                    .filter(|token| !token.is_empty())
                    .collect::<Vec<_>>();
                let placeholder_declaration = tokens.windows(2).any(|tokens| {
                    matches!(tokens[0], "struct" | "enum" | "type" | "mod" | "fn")
                        && matches!(tokens[1], "Built" | "Common" | "View")
                });
                assert!(
                    !placeholder_declaration,
                    "{}:{} declares a placeholder domain name",
                    path.display(),
                    line_index + 1
                );
            }
            assert!(
                !source.contains("SiglipConcept"),
                "{} restores the unconstrained SiglipConcept facade",
                path.display()
            );
        }
    }
}

#[test]
fn physical_io_and_control_share_one_compile_time_boundary() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = backend.parent().expect("workspace crates directory");
    let hardware = crates.join("spiral-robotics-hardware");
    assert!(hardware.join("src/lib.rs").is_file());
    for retired in ["spiral-robotics", "spiral-robotics-studio-hardware"] {
        assert!(
            !crates.join(retired).exists(),
            "retired hardware boundary {retired:?} must not return"
        );
    }
    assert!(
        !backend.join("src/robotics/hardware.rs").exists(),
        "physical I/O and fixed-rate control must not drift back into the delivery crate"
    );

    let manifest = fs::read_to_string(hardware.join("Cargo.toml")).expect("hardware manifest");
    for forbidden in [
        "spiral-robotics-api",
        "spiral-robotics-backend",
        "axum",
        "dioxus",
        "adbc",
        "arrow",
        "spiral-robotics-inference",
        "spiral-robotics-training",
    ] {
        assert!(
            !declares_local_dependency(&manifest, forbidden),
            "hardware crate depends on outward detail {forbidden:?}"
        );
    }

    let backend_manifest =
        fs::read_to_string(backend.join("Cargo.toml")).expect("backend manifest");
    assert!(
        declares_local_dependency(&backend_manifest, "spiral-robotics-hardware"),
        "the backend must compose the unified hardware crate through Cargo"
    );
}

#[test]
fn read_controllers_delegate_selection_and_admission_to_backend_capabilities() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http");
    let routes = fs::read_to_string(root.join("routes.rs")).expect("routes source");
    for seam in [
        "RobotCatalogActions::new(&robot_adapter)",
        "devices::CameraDiscoveryAdapter::new(state.cameras().clone())",
    ] {
        assert!(routes.contains(seam), "state snapshot bypasses {seam}");
    }

    let devices = fs::read_to_string(root.join("routes/devices.rs")).expect("device routes");
    for seam in [
        ".serial_ports()",
        "RobotCatalogActions::new(&adapter)",
        "RobotCatalogActions::new(&RobotHttpAdapter::new(&state))",
        ".get(id)\n            .await",
    ] {
        assert!(
            devices.contains(seam),
            "device read controller bypasses {seam}"
        );
    }

    let recordings =
        fs::read_to_string(root.join("routes/recordings.rs")).expect("recording routes");
    for seam in ["EpisodeQueries::new", ".authorize_asset("] {
        assert!(
            recordings.contains(seam),
            "episode read controller bypasses {seam}"
        );
    }
}

#[test]
fn episode_catalog_is_loaded_explicitly_from_spiral() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = backend.parent().expect("workspace crates directory");
    let contracts = fs::read_to_string(crates.join("spiral-robotics-api/src/status.rs"))
        .expect("contracts source");
    let snapshot = contracts
        .split("pub struct StudioSnapshot")
        .nth(1)
        .and_then(|source| source.split("/// Lifecycle of the backend services").next())
        .expect("StudioSnapshot declaration");
    assert!(
        !snapshot.contains("pub episodes:"),
        "episode catalog must not ride the live Studio snapshot"
    );
    let routes = fs::read_to_string(backend.join("src/http/routes.rs")).expect("routes source");
    for catalog_data in ["state.episodes.issues()", "list_publications"] {
        assert!(
            !routes.contains(catalog_data),
            "episode catalog data {catalog_data:?} must not ride the live Studio snapshot"
        );
    }

    let server =
        fs::read_to_string(backend.join("src/http/server.rs")).expect("backend server source");
    assert!(
        !server.contains("episodes.reload_if_changed()"),
        "the sole-writer episode directory must not be polled"
    );

    let frontend_state =
        fs::read_to_string(crates.join("spiral-robotics-frontend/src/activities/data/state.rs"))
            .expect("Data controller source");
    assert!(
        frontend_state.contains("api::Http::get::<DatasetCatalogResponse>(\"/api/datasets\")"),
        "the Data controller must load the dataset index separately"
    );
    assert!(
        frontend_state.contains("format!(\"/api/episodes?dataset_id={dataset_id}\")"),
        "the Data controller must scope its explicit episode catalog to one dataset"
    );
    let studio_state =
        fs::read_to_string(crates.join("spiral-robotics-frontend/src/shell/state.rs"))
            .expect("Studio state source");
    assert!(
        !studio_state.contains("episode_catalog"),
        "Studio-wide state must not own or eagerly load the Data episode catalog",
    );
    assert!(
        !backend.join("src/recording/dataset.rs").exists(),
        "the filesystem episode store must stay removed"
    );
    let episode_store = fs::read_to_string(crates.join("spiral-robotics-storage/src/episodes.rs"))
        .expect("Spiral episode store source");
    for retired in [
        "struct FileStamp",
        "fn stamps(&self)",
        "std::fs",
        "tempfile",
    ] {
        assert!(
            !episode_store.contains(retired),
            "the Spiral episode store must not retain filesystem detail {retired:?}"
        );
    }

    let data_state =
        fs::read_to_string(crates.join("spiral-robotics-frontend/src/activities/data/state.rs"))
            .expect("data controller source");
    assert!(
        !data_state.contains("TimeoutFuture::new(1_000)"),
        "publication status must not use the retired one-second poll"
    );
    assert!(
        !data_state.contains("/api/publications"),
        "publication status must travel with the explicit episode catalog"
    );
}

#[test]
fn listener_binding_precedes_concurrent_external_bootstrap() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let server =
        fs::read_to_string(backend.join("src/http/server.rs")).expect("backend server source");
    let bind = server
        .find("TcpListener::bind(bind).await")
        .expect("server must bind its listener");
    let bootstrap = server
        .find(".bootstrap_services(")
        .expect("server must start background bootstrap");
    assert!(
        bind < bootstrap,
        "external bootstrap must follow listener bind"
    );
    let critical_path = &server[..bind];
    for blocking_startup in [
        "load_studio_preferences()",
        "TrainingManager::open(",
        "refresh_blocking()).await",
        "auto_connect_saved_robots",
    ] {
        assert!(
            !critical_path.contains(blocking_startup),
            "listener startup performs external work {blocking_startup:?}"
        );
    }
    assert!(
        server.contains("tokio::join!(catalog, camera)")
            && server.contains(
                "tokio::join!(preferences, training, workflow, workflow_agent, rollout)",
            ),
        "independent startup I/O must progress concurrently"
    );

    let training = fs::read_to_string(backend.join("src/training/manager.rs"))
        .expect("training manager source");
    assert!(
        training.contains("tokio::join!(backend_probe, spiral_probe)"),
        "training readiness probes must run concurrently"
    );
}

#[test]
fn studio_has_one_spiral_persistence_boundary() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    for retired in ["src/recording/dataset.rs", "src/application/datasets.rs"] {
        assert!(
            !backend.join(retired).exists(),
            "retired filesystem storage module {retired:?} must stay removed"
        );
    }

    let server =
        fs::read_to_string(backend.join("src/http/server.rs")).expect("backend server source");
    for retired_argument in ["workspace: PathBuf", "episodes: PathBuf", "simulate: bool"] {
        assert!(
            !server.contains(retired_argument),
            "Studio must not restore process-local storage/simulation option {retired_argument:?}"
        );
    }
    assert!(
        server.contains("ProjectStore::new")
            && server.contains("RobotWorkspace::new(")
            && server.contains("EpisodeStore::new(")
            && server.contains("StorageManager::new("),
        "robots, episodes, and Studio state must share one Spiral project store"
    );
    for retired in ["spiral_uri:", "StoredSpiralSettings", "DataSettingsStore"] {
        assert!(
            !server.contains(retired),
            "the Spiral project selector must not restore {retired:?}"
        );
    }
    assert!(
        server.contains("BackendStateStore::open_default")
            && !server.contains("Spiral project is required"),
        "Studio must start without a project and recover its bootstrap pointer from backend state"
    );

    let storage = backend
        .parent()
        .expect("workspace crates directory")
        .join("spiral-robotics-storage");
    let settings =
        fs::read_to_string(storage.join("src/settings.rs")).expect("storage settings source");
    let bootstrap = settings
        .split_once("struct BackendState {")
        .and_then(|(_, source)| source.split_once("pub struct BackendStateStore"))
        .map(|(state, _)| state)
        .expect("backend state must remain an explicit narrow document");
    assert!(bootstrap.contains("project_id: String"));
    assert!(bootstrap.contains("spiral_endpoint: String"));
    assert!(bootstrap.contains("spiral_spfs_endpoint: String"));
    for forbidden in ["episodes:", "robots:", "training:", "rollout:", "video:"] {
        assert!(
            !bootstrap.contains(forbidden),
            "backend bootstrap state must not grow a project-data field {forbidden:?}"
        );
    }

    let project_store = fs::read_to_string(storage.join("src/project_store.rs"))
        .expect("Spiral project-store source");
    for retired in [
        "spiral:memory:",
        "memory: Arc",
        "settings: StoredSpiralSettings",
    ] {
        assert!(
            !project_store.contains(retired),
            "the project store must not retain alternate destination plumbing {retired:?}"
        );
    }

    let checkpoints = fs::read_to_string(
        backend
            .parent()
            .expect("workspace crates directory")
            .join("spiral-robotics-storage/src/checkpoints.rs"),
    )
    .expect("checkpoint boundary source");
    for retired in ["CheckpointSource", "register_run_source"] {
        assert!(
            !checkpoints.contains(retired),
            "checkpoints must inherit the selected Spiral destination instead of retaining {retired:?}"
        );
    }
}

#[test]
fn recording_and_spiral_connection_lifecycles_have_single_owners() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let routes = fs::read_to_string(backend.join("src/http/routes.rs")).expect("HTTP state source");
    let recordings =
        fs::read_to_string(backend.join("src/recording_session.rs")).expect("recording session");
    let lease = fs::read_to_string(backend.join("src/data/catalog_lease.rs"))
        .expect("catalog lease source");
    let data_routes =
        fs::read_to_string(backend.join("src/http/routes/data.rs")).expect("data routes");
    let connection =
        fs::read_to_string(backend.join("src/data/connection.rs")).expect("connection controller");

    assert!(!routes.contains("recording_video:"));
    assert!(!lease.contains("recording_episode"));
    for owned in [
        "RecordingSessionPhase",
        "catalog_lease: Arc<CatalogMutationLease>",
        "media: tokio::sync::Mutex<Option<VideoCapture>>",
    ] {
        assert!(
            recordings.contains(owned),
            "recording owner is missing {owned:?}"
        );
    }

    assert!(!data_routes.contains("backend_state()"));
    assert!(!data_routes.contains("configure_connection("));
    assert!(
        data_routes.contains("self.state")
            && data_routes.contains(".connection()")
            && data_routes.contains(".update(")
    );
    for owned in [
        "struct SpiralConnectionController",
        "current: Mutex<SpiralConnectionSettings>",
        "fn reload_project_state",
    ] {
        assert!(
            connection.contains(owned),
            "connection owner is missing {owned:?}"
        );
    }
}

#[test]
fn robot_mutations_are_serialized_with_project_reads_without_blocking_bridges() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = fs::read_to_string(backend.join("src/robots/workspace.rs"))
        .expect("robot workspace source");
    for (operation, next) in [
        ("pub(crate) async fn delete", "pub(crate) async fn try_list"),
        ("async fn persist", "\n}\n\ntrait PortConfigurationValue"),
    ] {
        let implementation = workspace
            .split_once(operation)
            .and_then(|(_, source)| source.split_once(next))
            .map(|(implementation, _)| implementation)
            .unwrap_or_else(|| panic!("robot workspace must define {operation}"));
        assert!(
            implementation.contains("connect_exclusive()")
                && implementation.contains(".await")
                && !implementation.contains("block_on"),
            "robot {operation} must make the next Studio snapshot read fresh state"
        );
    }

    let storage = backend
        .parent()
        .expect("workspace crates directory")
        .join("spiral-robotics-storage");
    let project_store = fs::read_to_string(storage.join("src/project_store.rs"))
        .expect("Spiral project-store source");
    let production_project_store = project_store
        .rsplit_once("#[cfg(test)]\nmod tests")
        .map_or(project_store.as_str(), |(production, _)| production);
    assert!(
        production_project_store.contains("async_operation_gate: Arc<tokio::sync::RwLock<()>>")
            && production_project_store.contains("write_owned().await")
            && production_project_store.contains("read_owned().await")
            && !production_project_store.contains("run_blocking")
            && !production_project_store.contains("block_on"),
        "a robot write must asynchronously drain project readers before opening its request-scoped native connection"
    );
}

#[test]
fn native_async_spiral_is_confined_to_one_adapter() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_root = backend
        .parent()
        .expect("workspace crates directory")
        .join("spiral-robotics-storage/src");
    let native_adapter_path = source_root.join("spiral_database.rs");
    let mut sources = Vec::new();
    collect_production_rust_sources(&source_root, &mut sources);
    for path in sources {
        if path == native_adapter_path {
            continue;
        }
        let source = fs::read_to_string(&path).expect("backend source must be readable");
        for implementation_detail in [
            "spiral_client::",
            "spiral_engine::",
            "spiral_session::",
            "vortex_array::",
            "vortex_arrow::",
        ] {
            assert!(
                !source.contains(implementation_detail),
                "{} bypasses the storage-owned native adapter with {implementation_detail}",
                path.display()
            );
        }
    }

    let native_adapter = fs::read_to_string(native_adapter_path)
        .expect("native Spiral adapter source must be readable");
    for native_boundary in [
        "impl Database for SpiralDatabase",
        "struct SpiralConnection",
        "ConnectionCtx",
        "collect_stream_with_options",
        "struct ArrowStreamSource",
    ] {
        assert!(
            native_adapter.contains(native_boundary),
            "native Spiral adapter is missing {native_boundary}"
        );
    }
    for retired_indirection in [
        "tokio::task::LocalSet",
        "spawn_local",
        "mpsc::",
        "oneshot::",
        "ConnectionRequest",
        "SpiralConnectionProxy",
    ] {
        assert!(
            !native_adapter.contains(retired_indirection),
            "native Spiral adapter retains retired indirection {retired_indirection:?}"
        );
    }
    let database = fs::read_to_string(source_root.join("database.rs"))
        .expect("async database contract must be readable");
    for contract in [
        "pub trait Database:",
        "pub trait DatabaseSession:",
        "pub enum DatabaseCommand",
        "pub struct RecordBatchStream",
    ] {
        assert!(
            database.contains(contract),
            "async database boundary is missing {contract}"
        );
    }
    let server = fs::read_to_string(backend.join("src/http/server.rs"))
        .expect("backend server source must be readable");
    assert!(
        !server.contains("LocalSet") && !server.contains("run_until("),
        "the server must run native Send database futures directly on its Tokio runtime"
    );
    for manifest_path in [
        backend.join("../../Cargo.toml"),
        backend.join("Cargo.toml"),
        backend
            .parent()
            .expect("workspace crates directory")
            .join("spiral-robotics-storage/Cargo.toml"),
    ] {
        let manifest = fs::read_to_string(&manifest_path).expect("manifest must be readable");
        assert!(
            !manifest.contains("adbc_core") && !manifest.contains("adbc-driver-spiraldb"),
            "{} retains the retired Rust ADBC dependency",
            manifest_path.display()
        );
    }
}

#[test]
fn async_database_use_cases_use_structured_tasks_and_explicit_transactions() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let storage = backend
        .parent()
        .expect("workspace crates directory")
        .join("spiral-robotics-storage/src");
    let project_store = fs::read_to_string(storage.join("project_store.rs"))
        .expect("project-store source must be readable");
    assert!(
        project_store.contains("pub async fn begin_transaction")
            && project_store.contains("pub async fn commit")
            && project_store.contains("pub async fn rollback")
            && project_store.contains("pub(crate) async fn reconnect")
            && !project_store.contains("set_autocommit"),
        "native transactions must have explicit terminal operations without the retired ADBC-style autocommit shim"
    );

    let mut storage_sources = Vec::new();
    collect_production_rust_sources(&storage, &mut storage_sources);
    for path in storage_sources {
        let source = fs::read_to_string(&path).expect("storage source must be readable");
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        for retired_bridge in [
            "tokio::spawn(",
            "spawn_blocking(",
            "block_on(",
            "block_in_place(",
            "blocking_send(",
            "run_blocking",
        ] {
            assert!(
                !production.contains(retired_bridge),
                "{} retains retired database bridge {retired_bridge:?}",
                path.display()
            );
        }
    }

    let episodes = fs::read_to_string(storage.join("episodes.rs"))
        .expect("episode storage source must be readable");
    assert!(
        !episodes.contains("EpisodeDatabaseWriter")
            && !episodes.contains("blocking_send")
            && !episodes.contains("tokio::spawn"),
        "the synchronous episode encoder must publish only an in-memory barrier; the async recording use case owns the durable commit"
    );

    let model = fs::read_to_string(backend.join("src/workflow/model.rs"))
        .expect("Workflow model source must be readable");
    let manager = fs::read_to_string(backend.join("src/workflow/manager.rs"))
        .expect("Workflow manager source must be readable");
    assert!(
        model.contains("tokio::join!(producer, computation)")
            && !model.contains("let producer = tokio::spawn"),
        "the async database producer and blocking model consumer must use structured concurrency"
    );
    assert!(
        manager.contains("tokio::join!(model, progress)")
            && !manager.contains("let event_task = tokio::spawn"),
        "Workflow model progress must be consumed as part of the owning run task"
    );
}

#[test]
fn backend_associated_types_are_only_closed_loop_capability_tokens() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed = BTreeSet::from([
        ("recording.rs".to_owned(), "MediaCapture".to_owned()),
        ("recording.rs".to_owned(), "MediaOutput".to_owned()),
        ("recording.rs".to_owned(), "RecordingDraft".to_owned()),
        ("service.rs".to_owned(), "CameraConfiguration".to_owned()),
    ]);

    let mut declared = BTreeSet::new();
    let mut sources = Vec::new();
    collect_production_rust_sources(&directory, &mut sources);
    for path in sources {
        let source = fs::read_to_string(&path).expect("backend source must be readable");
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("backend source has a UTF-8 file name");
        for line in production.lines() {
            let line = line.trim();
            if let Some(name) = line
                .strip_prefix("type ")
                .and_then(|declaration| declaration.split(';').next())
                .map(str::trim)
                .filter(|declaration| !declaration.contains('='))
            {
                declared.insert((file.to_owned(), name.to_owned()));
            }
            for (_owner, token) in &allowed {
                if line.contains(&format!("::{token}")) {
                    assert!(
                        line.contains(&format!("Self::{token}"))
                            || line.contains(&format!("P::{token}")),
                        "{} lets associated capability {token} escape its declaring use-case boundary: {line}",
                        path.display()
                    );
                }
            }
        }
    }

    assert_eq!(
        declared, allowed,
        "backend associated types must be an explicitly reviewed, adapter-minted and adapter-consumed capability"
    );
}

#[test]
fn workspace_crate_dependencies_point_toward_policy() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Studio lives under the workspace crates directory");
    let manifest = |relative: &str| {
        fs::read_to_string(workspace.join(relative))
            .unwrap_or_else(|error| panic!("could not read {relative}: {error}"))
    };

    let crates = [
        (
            "spiral-robotics-api",
            "crates/spiral-robotics-api/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-hardware",
            "crates/spiral-robotics-hardware/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-inference",
            "crates/spiral-robotics-inference/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-training",
            "crates/spiral-robotics-training/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-viewport",
            "crates/spiral-robotics-viewport/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-studio",
            "crates/spiral-robotics-studio/Cargo.toml",
            &["spiral-robotics-backend", "spiral-robotics-frontend"][..],
        ),
        (
            "spiral-robotics-frontend",
            "crates/spiral-robotics-frontend/Cargo.toml",
            &["spiral-robotics-api", "spiral-robotics-viewport"][..],
        ),
        (
            "spiral-robotics-storage",
            "crates/spiral-robotics-storage/Cargo.toml",
            &["spiral-robotics-api", "spiral-robotics-hardware"][..],
        ),
        (
            "spiral-robotics-backend",
            "crates/spiral-robotics-backend/Cargo.toml",
            &[
                "spiral-robotics-api",
                "spiral-robotics-hardware",
                "spiral-robotics-inference",
                "spiral-robotics-storage",
                "spiral-robotics-training",
            ][..],
        ),
    ];
    let local_names: Vec<_> = crates.iter().map(|(name, _, _)| *name).collect();

    for (name, path, allowed_dependencies) in crates {
        let source = manifest(path);
        for dependency in &local_names {
            if dependency == &name {
                continue;
            }
            assert_eq!(
                declares_local_dependency(&source, dependency),
                allowed_dependencies.contains(dependency),
                "{name} has an unexpected workspace dependency relationship with {dependency}"
            );
        }
    }
}

#[test]
fn studio_training_owns_only_the_remote_backend_trait_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Studio lives under the workspace crates directory");
    let manager =
        fs::read_to_string(root.join("crates/spiral-robotics-backend/src/training/manager.rs"))
            .expect("training manager source");
    let job_loop = manager
        .split_once("async fn run_backend")
        .and_then(|(_, source)| source.split_once("fn apply_remote_job_snapshot"))
        .map(|(source, _)| source)
        .expect("training manager must have one explicit worker-job loop");
    assert!(
        manager.contains("training_backend: Arc<dyn TrainingBackend>")
            && job_loop.contains("create_job(job_request)")
            && job_loop.contains("backend.job_snapshot(&next_run_id, cursor)")
            && job_loop.contains("backend.stop_job(&next_run_id, cursor)"),
        "Studio must orchestrate a backend-neutral worker job"
    );
    for forbidden_data_plane in [
        "run_training_adbc",
        "SpiralActLoader",
        "create_session",
        "buffer_training_batch",
        "TrainingBatch",
    ] {
        assert!(
            !job_loop.contains(forbidden_data_plane),
            "the active Studio job loop must not retain training data-plane detail {forbidden_data_plane:?}"
        );
    }
    for detail in [
        "NativeActTrainer",
        "VisionActTrainingConfig",
        "VisionActBatch",
        "tch::",
        "torch_sys",
    ] {
        assert!(
            !manager.contains(detail),
            "Studio training manager imports backend detail {detail:?}"
        );
    }

    let adapter = fs::read_to_string(root.join("training/src/spiral_training_runtime/act.py"))
        .expect("Python PyTorch training adapter source");
    assert!(
        adapter.contains("ACTPolicy") && adapter.contains("torch.optim.AdamW"),
        "the remote implementation must use the real PyTorch ACT policy and optimizer"
    );
    let spiral = fs::read_to_string(root.join("training/src/spiral_training_runtime/spiral.py"))
        .expect("Python Spiral training adapter source");
    for required in [
        "spiral.api_url",
        "spiral.spfs_url",
        "username",
        "password",
        "spql.video.frames",
        "spql.video.decode",
        "training_checkpoints",
    ] {
        assert!(
            spiral.contains(required),
            "the worker must own Spiral training operation {required:?}"
        );
    }
    for removed_native_crate in [
        "crates/spiral-robotics-inference-pytorch",
        "crates/spiral-robotics-training-pytorch",
        "crates/spiral-robotics-training-runtime",
    ] {
        assert!(
            !root.join(removed_native_crate).exists(),
            "obsolete native training crate {removed_native_crate:?} must stay removed"
        );
    }
}

#[test]
fn studio_rollout_does_not_enable_or_call_the_pytorch_backend() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Studio lives under the workspace crates directory");
    let manifest = fs::read_to_string(root.join("crates/spiral-robotics-backend/Cargo.toml"))
        .expect("Studio backend manifest");
    assert!(
        manifest
            .lines()
            .any(|line| line.trim() == "spiral-robotics-inference.workspace = true"),
        "Studio must consume only the featureless inference contract"
    );
    assert!(
        !manifest.contains("spiral-robotics-inference-pytorch"),
        "Studio must not depend on a native PyTorch inference implementation"
    );
    let inference_manifest =
        fs::read_to_string(root.join("crates/spiral-robotics-inference/Cargo.toml"))
            .expect("framework-neutral inference manifest");
    for native_dependency in ["tch", "torch-sys", "pytorch"] {
        assert!(
            !inference_manifest
                .to_ascii_lowercase()
                .contains(native_dependency),
            "framework-neutral inference contains native dependency {native_dependency:?}"
        );
    }
    for obsolete_build_script in [
        "crates/spiral-robotics-studio/build.rs",
        "crates/spiral-robotics-backend/build.rs",
    ] {
        assert!(
            !root.join(obsolete_build_script).exists(),
            "Studio must not inject tensor-runtime linker settings"
        );
    }

    let rollout =
        fs::read_to_string(root.join("crates/spiral-robotics-backend/src/rollout/manager.rs"))
            .expect("rollout manager source");
    assert!(
        rollout.contains("HttpPolicyLoader"),
        "Studio rollout must load policy sessions through the remote runtime"
    );
    for local_backend in [
        "spiral_robotics_inference::act",
        "load_cpu_artifact",
        "load_metal_artifact",
        "LibTorch",
    ] {
        assert!(
            !rollout.contains(local_backend),
            "Studio rollout contains local tensor backend detail {local_backend:?}"
        );
    }
}

#[test]
fn rollout_action_scheduler_contains_no_inline_inference_work() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rollout/manager.rs"))
            .expect("rollout manager source");
    let scheduler = source
        .split_once("async fn run_session(")
        .and_then(|(_, body)| body.split_once("    fn classify_control("))
        .map(|(body, _)| body)
        .expect("rollout session scheduler boundaries");

    assert!(
        scheduler.contains("observation.spawn_inference(next_policy)"),
        "the running action scheduler must hand inference to its worker"
    );
    assert!(
        scheduler.contains("actions.merge(completed.requested_timestep, &completed.chunk)")
            && scheduler.contains("let action = actions.pop()?"),
        "async rollout must merge timed chunks into the non-blocking action queue"
    );
    let dispatch = scheduler
        .find(".set_policy_goals(")
        .expect("the scheduler dispatches a physical action");
    let observe = scheduler
        .find("pending_observation.replace(observation)")
        .expect("the scheduler captures a latest-only observation");
    assert!(
        dispatch < observe
            && scheduler.contains("let latest_action = actions.latest_action_timestep()?")
            && scheduler.contains("initial.chunk.steps().refill_threshold()"),
        "LeRobot parity requires execute-then-observe timestamps and the default half-chunk refill threshold"
    );
    assert!(
        !scheduler.contains("policy.infer(")
            && !scheduler.contains("decode_images(")
            && !scheduler.contains("block_in_place"),
        "camera decoding and model execution must never run inline with physical action dispatch"
    );
}
