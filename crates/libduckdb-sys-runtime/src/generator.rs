use syn::{ForeignItem, Item, ItemForeignMod, parse_file};

pub fn parse_bindings(source: &str) -> Result<(Vec<Item>, Vec<syn::ForeignItemFn>), String> {
    let ast = parse_file(source).map_err(|error| error.to_string())?;
    split_bindings(ast.items)
}

pub fn split_bindings(items: Vec<Item>) -> Result<(Vec<Item>, Vec<syn::ForeignItemFn>), String> {
    let mut declarations = Vec::new();
    let mut functions = Vec::new();
    for item in items {
        match item {
            Item::ForeignMod(module) => split_foreign_module(module, &mut functions)?,
            Item::Verbatim(_) => {
                return Err("unsupported verbatim item in pinned DuckDB bindings".into());
            }
            item => declarations.push(item),
        }
    }
    if functions.is_empty() {
        return Err("pinned DuckDB function table must not be empty".into());
    }
    Ok((declarations, functions))
}

fn split_foreign_module(
    module: ItemForeignMod,
    functions: &mut Vec<syn::ForeignItemFn>,
) -> Result<(), String> {
    if module
        .abi
        .name
        .as_ref()
        .is_none_or(|name| name.value() != "C")
    {
        return Err("unsupported foreign ABI in pinned DuckDB bindings".into());
    }
    for item in module.items {
        match item {
            ForeignItem::Fn(function) => {
                if function.sig.variadic.is_some() {
                    return Err("variadic DuckDB functions are not supported".into());
                }
                for input in &function.sig.inputs {
                    let syn::FnArg::Typed(argument) = input else {
                        return Err("unexpected receiver in DuckDB C function".into());
                    };
                    if !matches!(argument.pat.as_ref(), syn::Pat::Ident(_)) {
                        return Err(format!(
                            "unsupported argument pattern for {}",
                            function.sig.ident
                        ));
                    }
                }
                functions.push(function);
            }
            _ => return Err("unexpected item in DuckDB extern block".into()),
        }
    }
    Ok(())
}
