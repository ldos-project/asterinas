/// The implementation of the `select!` macro.
/// 
/// XXX: This syntax is probably bad and will be replaced.

use proc_macro2::Span;
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

/// The implementations of the `select!` macro.
pub fn select_macro_impl(input: SelectInput) -> proc_macro2::TokenStream {
    let blockers: Vec<_> = input
        .clauses
        .iter()
        .map(|clause| clause.blocker())
        .collect();

    // Generate all the check statements which run each time a blocker wakes.
    let check_statements = input.clauses.iter().map(|clause| {
        let attrs = &clause.let_binding.attrs;
        let pat = &clause.let_binding.pat;
        let blocker_expr = &clause.let_binding.expr;
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
            ::orpc::sync::task::Task::current().block_on(&[#(::std::convert::AsRef::as_ref(&#blockers)),*]);
            #(#check_statements)*
        }
    };
    output
}
