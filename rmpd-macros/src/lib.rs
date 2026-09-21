//! Derive macros for rmpd `Command` metadata.
//!
//! Provides `#[derive(CommandMetadata)]` which generates `command_name()` and
//! `command_required_permission()` methods, plus a free `command_arity()`
//! function, from `#[command(...)]` attributes on each enum variant.
//!
//! # Attribute syntax
//!
//! ```ignore
//! #[command(name = "play", permission = 4, args = "0..1")]
//! Play { position: Option<u32> },
//! ```
//!
//! - `name` (required): the MPD wire name of the command.
//! - `permission` (optional, default `0`): the required permission bitmask (`u8`).
//! - `args` (optional): `"MIN..MAX"` argument-count bounds fed into the generated
//!   `command_arity()` lookup (`MAX == -1` means unlimited). Commands with no
//!   `args` on any of their variants are omitted from the table (looked up as
//!   `None`). Only one variant per command name needs to carry `args`; every
//!   variant sharing that name must agree if more than one specifies it.

use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashMap;
use syn::{Data, DeriveInput, Fields, Lit, parse_macro_input};

/// Derive `command_name(&self) -> &'static str`,
/// `command_required_permission(&self) -> u8`, and a free
/// `command_arity(name: &str) -> Option<(i32, i32)>` from `#[command(...)]`
/// attributes.
#[proc_macro_derive(CommandMetadata, attributes(command))]
pub fn derive_command_metadata(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let variants = match &input.data {
        Data::Enum(data) => &data.variants,
        _ => {
            return syn::Error::new_spanned(
                &input,
                "CommandMetadata can only be derived for enums",
            )
            .to_compile_error()
            .into();
        }
    };

    let mut name_arms = Vec::new();
    let mut perm_arms = Vec::new();

    let mut arity_map: HashMap<String, (i32, i32)> = HashMap::new();
    let mut arity_order: Vec<String> = Vec::new();

    for variant in variants {
        let ident = &variant.ident;

        let mut cmd_name: Option<String> = None;
        let mut cmd_perm: u8 = 0;
        let mut cmd_args: Option<(i32, i32)> = None;
        for attr in &variant.attrs {
            if !attr.path().is_ident("command") {
                continue;
            }

            if let Err(e) = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let value = meta.value()?;
                    let lit: Lit = value.parse()?;
                    if let Lit::Str(s) = lit {
                        cmd_name = Some(s.value());
                    } else {
                        return Err(meta.error("expected string literal for `name`"));
                    }
                } else if meta.path.is_ident("permission") {
                    let value = meta.value()?;
                    let lit: Lit = value.parse()?;
                    if let Lit::Int(i) = lit {
                        cmd_perm = i.base10_parse()?;
                    } else {
                        return Err(meta.error("expected integer literal for `permission`"));
                    }
                } else if meta.path.is_ident("args") {
                    let value = meta.value()?;
                    let lit: Lit = value.parse()?;
                    if let Lit::Str(s) = lit {
                        let spec = s.value();
                        let (min_str, max_str) = spec
                            .split_once("..")
                            .ok_or_else(|| meta.error("expected `MIN..MAX` for `args`"))?;
                        let min: i32 = min_str
                            .parse()
                            .map_err(|_| meta.error("invalid `MIN` in `args`"))?;
                        let max: i32 = max_str
                            .parse()
                            .map_err(|_| meta.error("invalid `MAX` in `args`"))?;
                        cmd_args = Some((min, max));
                    } else {
                        return Err(meta.error("expected string literal for `args`"));
                    }
                } else {
                    return Err(meta
                        .error("unknown attribute key; expected `name`, `permission`, or `args`"));
                }
                Ok(())
            }) {
                return e.to_compile_error().into();
            }
        }

        let cmd_name = match cmd_name {
            Some(n) => n,
            None => {
                return syn::Error::new_spanned(
                    variant,
                    format!(
                        "variant `{}` is missing `#[command(name = \"...\")]` attribute",
                        ident
                    ),
                )
                .to_compile_error()
                .into();
            }
        };

        let pattern = match &variant.fields {
            Fields::Unit => quote! { Self::#ident },
            Fields::Named(_) => quote! { Self::#ident { .. } },
            Fields::Unnamed(_) => quote! { Self::#ident(..) },
        };

        name_arms.push(quote! { #pattern => #cmd_name, });
        perm_arms.push(quote! { #pattern => #cmd_perm, });

        if let Some(arity) = cmd_args {
            match arity_map.get(&cmd_name) {
                Some(existing) if *existing != arity => {
                    return syn::Error::new_spanned(
                        variant,
                        format!(
                            "command `{}` has conflicting `args` values across variants",
                            cmd_name
                        ),
                    )
                    .to_compile_error()
                    .into();
                }
                Some(_) => {}
                None => {
                    arity_map.insert(cmd_name.clone(), arity);
                    arity_order.push(cmd_name.clone());
                }
            }
        }
    }

    let arity_arms = arity_order.iter().map(|n| {
        let (min, max) = arity_map[n];
        quote! { #n => Some((#min, #max)), }
    });

    let expanded = quote! {
        impl #name {
            /// Return the MPD wire name of this command (for ACK error messages).
            pub fn command_name(&self) -> &'static str {
                match self {
                    #(#name_arms)*
                }
            }

            /// Return the required permission bitmask for this command.
            pub fn command_required_permission(&self) -> u8 {
                match self {
                    #(#perm_arms)*
                }
            }
        }

        /// Per-command `(min, max)` argument-count bounds, generated from the
        /// `#[command(args = "...")]` attributes above (`max == -1` means
        /// unlimited). Commands with no `args` attribute return `None`.
        fn command_arity(name: &str) -> Option<(i32, i32)> {
            match name {
                #(#arity_arms)*
                _ => None,
            }
        }
    };

    TokenStream::from(expanded)
}
