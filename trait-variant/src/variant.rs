// Copyright (c) 2023 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::iter;

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, parse_quote,
    punctuated::Punctuated,
    token::Plus,
    FnArg, Ident, ItemTrait, Pat, PatType, Path, Result, ReturnType, Signature, Token, TraitBound,
    TraitBoundModifier, TraitItem, TraitItemFn, Type, TypeImplTrait, TypeParamBound,
};
use syn::{PatIdent, Receiver, TypeReference, WhereClause};

struct Attrs {
    variant: MakeVariant,
}

impl Parse for Attrs {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(Self {
            variant: MakeVariant::parse(input)?,
        })
    }
}

struct MakeVariant {
    name: Ident,
    #[allow(unused)]
    colon: Token![:],
    bounds: Punctuated<TraitBound, Plus>,
}

impl Parse for MakeVariant {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(Self {
            name: input.parse()?,
            colon: input.parse()?,
            bounds: input.parse_terminated(TraitBound::parse, Token![+])?,
        })
    }
}

pub fn only_send(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let item = parse_macro_input!(item as ItemTrait);
    let ident = &item.ident;
    let attrs = Attrs {
        variant: syn::parse2(quote! { #ident: Send }).unwrap(),
    };
    let variant = mk_variant(&attrs, &item);
    quote! {
        #variant
    }
    .into()
}

fn mk_variant(attrs: &Attrs, tr: &ItemTrait) -> TokenStream {
    let MakeVariant {
        ref name,
        colon: _,
        ref bounds,
    } = attrs.variant;
    let bounds: Vec<_> = bounds
        .into_iter()
        .map(|b| TypeParamBound::Trait(b.clone()))
        .collect();
    let variant = ItemTrait {
        ident: name.clone(),
        supertraits: tr.supertraits.iter().chain(&bounds).cloned().collect(),
        items: tr
            .items
            .iter()
            .map(|item| transform_item(item, &bounds))
            .collect(),
        ..tr.clone()
    };
    quote! { #variant }
}

fn transform_item(item: &TraitItem, bounds: &Vec<TypeParamBound>) -> TraitItem {
    // #[make_variant(SendIntFactory: Send)]
    // trait IntFactory {
    //     async fn make(&self, x: u32, y: &str) -> i32;
    //     fn stream(&self) -> impl Iterator<Item = i32>;
    //     fn call(&self) -> u32;
    // }
    //
    // becomes:
    //
    // trait SendIntFactory: Send {
    //     fn make(&self, x: u32, y: &str) -> impl ::core::future::Future<Output = i32> + Send;
    //     fn stream(&self) -> impl Iterator<Item = i32> + Send;
    //     fn call(&self) -> u32;
    // }
    let TraitItem::Fn(fn_item @ TraitItemFn { sig, default, .. }) = item else {
        return item.clone();
    };
    let (sig, default) = if sig.asyncness.is_some() {
        let orig = match &sig.output {
            ReturnType::Default => quote! { () },
            ReturnType::Type(_, ty) => quote! { #ty },
        };
        let future = syn::parse2(quote! { ::core::future::Future<Output = #orig> }).unwrap();
        let ty = Type::ImplTrait(TypeImplTrait {
            impl_token: syn::parse2(quote! { impl }).unwrap(),
            bounds: iter::once(TypeParamBound::Trait(future))
                .chain(bounds.iter().cloned())
                .collect(),
        });
        let mut sig = sig.clone();
        if default.is_some() {
            add_receiver_bounds(&mut sig);
        }

        (
            Signature {
                asyncness: None,
                output: ReturnType::Type(syn::parse2(quote! { -> }).unwrap(), Box::new(ty)),
                ..sig.clone()
            },
            fn_item.default.as_ref().map(|b| {
                let items = sig.inputs.iter().map(|i| match i {
                    FnArg::Receiver(Receiver { self_token, .. }) => {
                        quote! { let __self = #self_token; }
                    }
                    FnArg::Typed(PatType { pat, .. }) => match pat.as_ref() {
                        Pat::Ident(PatIdent { ident, .. }) => quote! { let #ident = #ident; },
                        _ => todo!(),
                    },
                });

                struct ReplaceSelfVisitor;
                impl syn::visit_mut::VisitMut for ReplaceSelfVisitor {
                    fn visit_ident_mut(&mut self, ident: &mut syn::Ident) {
                        if ident == "self" {
                            *ident = syn::Ident::new("__self", ident.span());
                        }
                        syn::visit_mut::visit_ident_mut(self, ident);
                    }
                }

                let mut block = b.clone();
                syn::visit_mut::visit_block_mut(&mut ReplaceSelfVisitor, &mut block);

                parse_quote! { { async move { #(#items)* #block} } }
            }),
        )
    } else {
        match &sig.output {
            ReturnType::Type(arrow, ty) => match &**ty {
                Type::ImplTrait(it) => {
                    let ty = Type::ImplTrait(TypeImplTrait {
                        impl_token: it.impl_token,
                        bounds: it.bounds.iter().chain(bounds).cloned().collect(),
                    });
                    (
                        Signature {
                            output: ReturnType::Type(*arrow, Box::new(ty)),
                            ..sig.clone()
                        },
                        fn_item.default.clone(),
                    )
                }
                _ => return item.clone(),
            },
            ReturnType::Default => return item.clone(),
        }
    };
    TraitItem::Fn(TraitItemFn {
        sig,
        default,
        ..fn_item.clone()
    })
}

fn add_receiver_bounds(sig: &mut Signature) {
    let Some(FnArg::Receiver(Receiver { ty, reference, .. })) = sig.inputs.first_mut() else {
        return;
    };
    let Type::Reference(
        recv_ty @ TypeReference {
            mutability: None, ..
        },
    ) = &mut **ty
    else {
        return;
    };
    let Some((_and, lt)) = reference else {
        return;
    };

    let lifetime = syn::Lifetime {
        apostrophe: Span::mixed_site(),
        ident: Ident::new("the_self_lt", Span::mixed_site()),
    };
    sig.generics.params.insert(
        0,
        syn::GenericParam::Lifetime(syn::LifetimeParam {
            lifetime: lifetime.clone(),
            colon_token: None,
            bounds: Default::default(),
            attrs: Default::default(),
        }),
    );
    recv_ty.lifetime = Some(lifetime.clone());
    *lt = Some(lifetime);
    let predicate = parse_quote! { #recv_ty: Send };

    if let Some(wh) = &mut sig.generics.where_clause {
        wh.predicates.push(predicate);
    } else {
        let where_clause = WhereClause {
            where_token: Token![where](Span::mixed_site()),
            predicates: Punctuated::from_iter([predicate]),
        };
        sig.generics.where_clause = Some(where_clause);
    }
}
