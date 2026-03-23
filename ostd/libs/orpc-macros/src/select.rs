// SPDX-License-Identifier: MPL-2.0
/// The implementation of the `select_legacy!` macro.
///
/// TODO(#73): This syntax is probably bad and will be replaced.
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Block, Expr, ExprLet, Ident, Token, parse::Parse, punctuated::Punctuated, token::Comma};

/// A syn-parsable struct for the syntax:
///
/// ```ignore
/// if let Pat(..) = expr { body }
/// ```
struct BlockerClause {
    #[expect(unused)]
    pub if_token: Token![if],
    pub let_binding: ExprLet,
    pub body: Block,
}

impl BlockerClause {
    /// Get the blocker from the bound expr. This is the receiver, the struct holding the member, or the first argument
    /// of the function call.
    fn blocker(&self) -> &Expr {
        let rhs = &self.let_binding.expr;
        match rhs.as_ref() {
            Expr::Field(expr_field) => expr_field.base.as_ref(),
            Expr::MethodCall(expr_method_call) => expr_method_call.receiver.as_ref(),
            Expr::Call(expr_call) => expr_call.args.first().unwrap_or(rhs),
            _ => rhs,
        }
    }
}

impl Parse for BlockerClause {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(BlockerClause {
            if_token: input.parse()?,
            let_binding: input.parse()?,
            body: input.parse()?,
        })
    }
}

/// A syn-parsable sequence of BlockerClauses.
pub struct SelectInput {
    clauses: Punctuated<BlockerClause, Comma>,
}

impl Parse for SelectInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(SelectInput {
            clauses: Punctuated::parse_terminated(input)?,
        })
    }
}

/// The implementation of the `select!` and `select_legacy!` macros. The `wrap_*` functions are used
/// for the slightly different reference and error handling in the two cases.
pub fn select_macro_impl(
    input: SelectInput,
    wrap_expr: impl Fn(&Expr) -> TokenStream,
    wrap_blocker: impl Fn(&Expr) -> TokenStream,
) -> TokenStream {
    let blockers: Vec<_> = input
        .clauses
        .iter()
        .map(|clause| wrap_blocker(clause.blocker()))
        .collect();

    // Generate all the check statements which run each time a blocker wakes.
    let check_statements = input.clauses.iter().map(|clause| {
        let attrs = &clause.let_binding.attrs;
        let pat = &clause.let_binding.pat;
        let blocker_expr = wrap_expr(&clause.let_binding.expr);
        let body = &clause.body;
        let tmp = Ident::new("message", Span::mixed_site());
        quote! {
            if #(#attrs)* let Some(#tmp) = #blocker_expr {
                // We put the user pattern in a separate binding to make sure it is irrefutable. If it isn't then
                // messages can be silently dropped.
                let #pat = #tmp;
                #body
            }
        }
    });

    let output = quote! {
        {
            ::ostd::task::Task::current().map(|c| c.block_on(&[#(#blockers),*]));
            #(#check_statements)*
        }
    };
    output
}
