//! Derive macros for rmpd `Command` metadata.
//!
//! Provides `#[derive(CommandMetadata)]` which generates `command_name()` and
//! `command_required_permission()` methods from `#[command(...)]` attributes on
//! each enum variant.
//!
//! # Attribute syntax
//!
//! ```ignore
//! #[command(name = "play", permission = 4)]
//! Play { position: Option<u32> },
//! ```
//!
//! - `name` (required): the MPD wire name of the command.
//! - `permission` (optional, default `0`): the required permission bitmask (`u8`).

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Lit, parse_macro_input};

/// Derive `command_name(&self) -> &'static str` and
/// `command_required_permission(&self) -> u8` from `#[command(...)]` attributes.
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

    for variant in variants {
        let ident = &variant.ident;

        let mut cmd_name: Option<String> = None;
        let mut cmd_perm: u8 = 0;

        for attr in &variant.attrs {
            if !attr.path().is_ident("command") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
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
                } else {
                    return Err(
                        meta.error("unknown attribute key; expected `name` or `permission`")
                    );
                }
                Ok(())
            })
            .unwrap_or_else(|e| {
                panic!("{}", e);
            });
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
    }

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
    };

    TokenStream::from(expanded)
}
