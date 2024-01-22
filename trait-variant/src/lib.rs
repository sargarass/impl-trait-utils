// Copyright (c) 2023 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![doc = include_str!("../README.md")]

mod variant;

/// Creates a specialized version of a base trait that adds Send bounds to `async
/// fn` and/or `-> impl Trait` return types.
///
/// ```
/// #[trait_variant::only_send]
/// trait IntFactory {
///     async fn make(&self) -> i32;
///     fn stream(&self) -> impl Iterator<Item = i32>;
///     fn call(&self) -> u32;
/// }
/// ```
///
/// The above example causes the trait to be rewritten as:
///
/// ```
/// # use core::future::Future;
/// trait IntFactory: Send {
///     fn make(&self) -> impl Future<Output = i32> + Send;
///     fn stream(&self) -> impl Iterator<Item = i32> + Send;
///     fn call(&self) -> u32;
/// }
/// ```
///
/// Note that ordinary methods such as `call` are not affected.
#[proc_macro_attribute]
pub fn only_send(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    variant::only_send(item)
}
