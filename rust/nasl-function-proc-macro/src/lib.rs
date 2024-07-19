mod error;
mod utils;

use error::Result;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    parenthesized, parse::Parse, parse_macro_input, punctuated::Punctuated, FnArg, Ident,
    ItemFn, Signature, Token, Type,
};
use utils::subty_if_name;

#[proc_macro_attribute]
pub fn nasl_function(
    attrs: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let function = parse_macro_input!(input as syn::ItemFn);
    let attrs = parse_macro_input!(attrs as Attrs);
    nasl_function_internal(function, attrs)
        .unwrap_or_else(|e| e.emit().into())
        .into()
}

fn nasl_function_internal(function: ItemFn, attrs: Attrs) -> Result<TokenStream> {
    let args = ArgsStruct::try_parse(&function, &attrs)?;
    Ok(args.impl_nasl_function_args())
}

mod attrs {
    syn::custom_keyword!(named);
    syn::custom_keyword!(maybe_named);
}

struct Attr {
    kind: AttrKind,
    ident: Ident,
}

enum AttrKind {
    Named,
    MaybeNamed,
}

impl Parse for Attr {
    fn parse(stream: syn::parse::ParseStream) -> syn::Result<Self> {
        let lookahead = stream.lookahead1();
        let kind = if lookahead.peek(attrs::named) {
            let _: attrs::named = stream.parse()?;
            Ok(AttrKind::Named)
        } else if lookahead.peek(attrs::maybe_named) {
            let _: attrs::maybe_named = stream.parse()?;
            Ok(AttrKind::MaybeNamed)
        } else {
            Err(lookahead.error())
        }?;
        let content;
        let _ = parenthesized!(content in stream);
        Ok(Self {
            kind,
            ident: content.parse()?,
        })
    }
}

struct Attrs {
    attrs: Vec<Attr>,
}

impl Attrs {
    fn get_arg_kind(&self, ident: &Ident, position: usize) -> ArgKind {
        let attr_kind = self.attrs.iter().find(|attr| &attr.ident == ident).map(|attr| &attr.kind);
        let make_named = || NamedArg {
            name: ident.to_string(),
        };
        let make_positional = || PositionalArg { position };
        match attr_kind {
            None => ArgKind::Positional(make_positional()),
            Some(AttrKind::Named) => ArgKind::Named(make_named()),
            Some(AttrKind::MaybeNamed) => ArgKind::MaybeNamed(make_positional(), make_named()),
        }
    }
}

impl Parse for Attrs {
    fn parse(stream: syn::parse::ParseStream) -> syn::Result<Self> {
        let attrs: Punctuated<Attr, Token![,]> = stream.parse_terminated(Attr::parse, Token![,])?;
        Ok(Self {
            attrs: attrs.into_iter().collect(),
        })
    }
}

#[derive(Debug)]
struct ArgsStruct<'a> {
    function: &'a ItemFn,
    args: Vec<Arg<'a>>,
}

#[derive(Debug)]
struct Arg<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    optional: bool,
    kind: ArgKind,
}

#[derive(Debug)]
enum ArgKind {
    Positional(PositionalArg),
    Named(NamedArg),
    MaybeNamed(PositionalArg, NamedArg),
}

#[derive(Debug)]
struct NamedArg {
    name: String,
}

#[derive(Debug)]
struct PositionalArg {
    position: usize,
}

impl<'a> Arg<'a> {
    fn new(arg: &'a FnArg, attrs: &Attrs, position: usize) -> Result<Self> {
        let (ident, ty, optional) = get_arg_info(arg)?;
        let kind = attrs.get_arg_kind(ident, position);
        Ok(Self {
            kind,
            ident,
            ty,
            optional,
        })
    }
}

fn get_arg_info(arg: &FnArg) -> Result<(&Ident, &Type, bool)> {
    match arg {
        FnArg::Receiver(_) => panic!(),
        FnArg::Typed(typed) => {
            let ident = match typed.pat.as_ref() {
                syn::Pat::Ident(ident) => &ident.ident,
                _ => panic!(),
            };
            let ty = &typed.ty;
            let (optional, ty) = if let Some(ty) = subty_if_name(ty, "Option") {
                (true, ty)
            } else {
                (false, ty.as_ref())
            };
            Ok((ident, ty, optional))
        }
    }
}

impl<'a> ArgsStruct<'a> {
    fn try_parse(function: &'a ItemFn, attrs: &'a Attrs) -> Result<Self> {
        Ok(Self {
            function: function,
            args: function
                .sig
                .inputs
                .iter()
                .enumerate()
                .map(|(position, arg)| Arg::new(arg, attrs, position))
                .collect::<Result<Vec<_>>>()?,
        })
    }

    fn positional(&self) -> impl Iterator<Item = (&Arg<'a>, &PositionalArg)> + '_ {
        self.args.iter().filter_map(|arg| match arg.kind {
            ArgKind::Positional(ref positional) => Some((arg, positional)),
            _ => None,
        })
    }

    fn num_required_positional(&self) -> usize {
        self.positional().filter(|(arg, _)| !arg.optional).count()
    }

    fn impl_nasl_function_args(&self) -> TokenStream {
        let ItemFn {
            attrs,
            vis,
            sig,
            block,
        } = self.function;
        let stmts = &block.stmts;
        let args = self.get_args();
        let Signature {
            fn_token,
            ident,
            generics,
            output,
            ..
        } = sig;
        let inputs = quote! {
            register: &::nasl_builtin_utils::Register,
            context: &::nasl_builtin_utils::Context,
        };
        quote! {
            #(#attrs)* #vis #fn_token #ident #generics ( #inputs ) #output {
                #args
                #(#stmts)*
            }
        }
    }

    fn get_args(&self) -> TokenStream {
        self
            .args.iter().map(|arg| {
                let num_required_positional_args = self.num_required_positional();
                let ident = &arg.ident;
                let ty = &arg.ty;
                let parse = match &arg.kind {
                    ArgKind::Positional(positional) => {
                        let position = positional.position;
                            if arg.optional {
                                quote! { ::nasl_builtin_utils::function::utils::get_optional_positional_arg::<#ty>(register, #position)? }
                            }
                            else {
                                quote! { ::nasl_builtin_utils::function::utils::get_positional_arg::<#ty>(register, #position, #num_required_positional_args)? }
                            }
                    }
                    ArgKind::Named(named) => {
                        let name = &named.name;
                        if arg.optional {
                            quote! { ::nasl_builtin_utils::function::utils::get_optional_named_arg::<#ty>(register, #name)? }
                        }
                        else {
                            quote! { ::nasl_builtin_utils::function::utils::get_named_arg::<#ty>(register, #name)? }
                        }
                    }
                    ArgKind::MaybeNamed(positional, named) => {
                        let name = &named.name;
                        let position = positional.position;
                        if arg.optional {
                            quote! {
                                ::nasl_builtin_utils::function::utils::get_optional_maybe_named_arg::<#ty>(register, #name, #position)?
                            }
                        }
                        else {
                            quote! {
                                ::nasl_builtin_utils::function::utils::get_maybe_named_arg::<#ty>(register, #name, #position)?
                            }
                        }
                    }
                };
                quote! {
                    let #ident = #parse;
                }
            })
            .collect()
    }
}
