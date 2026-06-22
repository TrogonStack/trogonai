//! Proc macros for exporting Trogon deciders as WASM components.

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{Token, parse_macro_input, punctuated::Punctuated};

/// Export one or more [`Decider`](trogon_decider::Decider) implementations as a WASM component.
#[proc_macro]
pub fn export_decider(input: TokenStream) -> TokenStream {
    let commands = parse_macro_input!(input with Punctuated<CommandSpec, Token![,]>::parse_terminated);
    if commands.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "export_decider! requires at least one command",
        )
        .to_compile_error()
        .into();
    }

    let canonical = &commands[0];
    let canonical_ty = &canonical.ty;
    let module_name = &canonical.module;
    let module_version = &canonical.version;
    let state_schema_version = &canonical.state_schema_version;

    let stream_id_arms = commands.iter().map(|command| {
        let ty = &command.ty;
        let type_url = &command.type_url;
        let proto = &command.proto;
        quote! {
            #type_url => {
                let command = ::trogon_decider_guest_sdk::decode_command::<
                    __trogon_decider_bindings::CommandEnvelope,
                    #proto,
                    #ty,
                    __trogon_decider_bindings::DomainError,
                >(#type_url, envelope)?;
                Ok(<#ty as ::trogon_decider::Decider>::stream_id(&command).to_string())
            }
        }
    });

    let decide_arms = commands.iter().map(|command| {
        let ty = &command.ty;
        let type_url = &command.type_url;
        let proto = &command.proto;
        quote! {
            #type_url => {
                let command = ::trogon_decider_guest_sdk::decode_command::<
                    __trogon_decider_bindings::CommandEnvelope,
                    #proto,
                    #ty,
                    __trogon_decider_bindings::DomainError,
                >(#type_url, envelope)
                .map_err(__trogon_decider_bindings::DecideError::Faulted)?;
                ::trogon_decider_guest_sdk::decide_command::<
                    #ty,
                    __trogon_decider_bindings::DecideError,
                    __trogon_decider_bindings::AnyEnvelope,
                >(&command, &*self.state.borrow())
            }
        }
    });

    let command_specs = commands.iter().map(|command| {
        let type_url = &command.type_url;
        let ty = &command.ty;
        let write_precondition = match &command.write_precondition {
            WritePreconditionSpec::Any => {
                quote! { ::trogon_decider_guest_sdk::map_write_precondition(Some(::trogon_decider::WritePrecondition::Any)) }
            }
            WritePreconditionSpec::StreamExists => {
                quote! { ::trogon_decider_guest_sdk::map_write_precondition(Some(::trogon_decider::WritePrecondition::StreamExists)) }
            }
            WritePreconditionSpec::NoStream => {
                quote! { ::trogon_decider_guest_sdk::map_write_precondition(Some(::trogon_decider::WritePrecondition::NoStream)) }
            }
            WritePreconditionSpec::Default => {
                quote! { ::trogon_decider_guest_sdk::map_write_precondition(<#ty as ::trogon_decider::Decider>::WRITE_PRECONDITION) }
            }
        };
        quote! {
            __trogon_decider_bindings::CommandSpec {
                command_type: (#type_url).to_string(),
                write_precondition: __trogon_decider_bindings::map_write_precondition_tag(#write_precondition),
            }
        }
    });

    let bundle_bounds = if commands.len() == 1 {
        quote! {}
    } else {
        let bounds = commands.iter().skip(1).map(|command| {
            let ty = &command.ty;
            quote! {
                #ty: ::trogon_decider::Decider<
                    State = <#canonical_ty as ::trogon_decider::Decider>::State,
                    Event = <#canonical_ty as ::trogon_decider::Decider>::Event,
                >,
            }
        });
        quote! { where #(#bounds)* }
    };

    let expanded = quote! {
        #[doc(hidden)]
        mod __trogon_decider_bindings {
            wit_bindgen::generate!({
                world: "decider",
                path: "../trogon-decider-wit/wit",
                generate_all,
            });

            use ::trogon_decider_guest_sdk::{
                AnyEnvelopeParts, AnyEnvelopeView, CommandEnvelopeView, DecideErrorView, DomainErrorParts,
                WritePreconditionTag,
            };
            pub use exports::trogon::decider::handler::{
                AnyEnvelope, CommandEnvelope, CommandSpec, DecideError, DomainError, Guest, GuestSession,
                ModuleDescriptor, WritePrecondition,
            };

            impl From<DomainErrorParts> for DomainError {
                fn from(value: DomainErrorParts) -> Self {
                    Self {
                        code: value.code,
                        message: value.message,
                    }
                }
            }

            impl DecideErrorView for DecideError {
                fn rejected(parts: DomainErrorParts) -> Self {
                    Self::Rejected(parts.into())
                }

                fn faulted(parts: DomainErrorParts) -> Self {
                    Self::Faulted(parts.into())
                }
            }

            impl CommandEnvelopeView for CommandEnvelope {
                fn command_type(&self) -> &str {
                    self.type_.as_str()
                }

                fn command_payload(&self) -> &[u8] {
                    &self.payload
                }
            }

            impl AnyEnvelopeView for AnyEnvelope {
                fn event_type(&self) -> &str {
                    self.type_.as_str()
                }

                fn event_payload(&self) -> &[u8] {
                    &self.payload
                }
            }

            impl From<AnyEnvelopeParts> for AnyEnvelope {
                fn from(value: AnyEnvelopeParts) -> Self {
                    Self {
                        type_: value.type_url,
                        payload: value.payload,
                    }
                }
            }

            pub fn map_write_precondition_tag(
                tag: Option<WritePreconditionTag>,
            ) -> Option<WritePrecondition> {
                tag.map(|value| match value {
                    WritePreconditionTag::Any => WritePrecondition::Any,
                    WritePreconditionTag::StreamExists => WritePrecondition::StreamExists,
                    WritePreconditionTag::NoStream => WritePrecondition::NoStream,
                })
            }
        }

        struct Component;

        struct Session {
            state: ::core::cell::RefCell<<#canonical_ty as ::trogon_decider::Decider>::State>,
        }

        impl __trogon_decider_bindings::Guest for Component {
            fn descriptor() -> __trogon_decider_bindings::ModuleDescriptor {
                __trogon_decider_bindings::ModuleDescriptor {
                    name: #module_name.to_string(),
                    version: #module_version.to_string(),
                    commands: vec![#(#command_specs),*],
                }
            }

            fn stream_id(
                command: __trogon_decider_bindings::CommandEnvelope,
            ) -> Result<String, __trogon_decider_bindings::DomainError> {
                let envelope = command;
                match envelope.type_.as_str() {
                    #(#stream_id_arms,)*
                    other => Err(__trogon_decider_bindings::DomainError {
                        code: "invalid-command".to_string(),
                        message: format!("unknown command type '{other}'"),
                    }),
                }
            }

            type Session = Session;
        }

        impl __trogon_decider_bindings::GuestSession for Session #bundle_bounds {
            fn new(snapshot: Option<Vec<u8>>) -> Self {
                let state = ::trogon_decider_guest_sdk::load_or_initial::<
                    #canonical_ty,
                    <#canonical_ty as ::trogon_decider::Decider>::State,
                >(snapshot, #state_schema_version);
                Self {
                    state: ::core::cell::RefCell::new(state),
                }
            }

            fn evolve(
                &self,
                events: Vec<__trogon_decider_bindings::AnyEnvelope>,
            ) -> Result<(), __trogon_decider_bindings::DomainError> {
                let mut state = self.state.borrow().clone();
                for envelope in events {
                    state = ::trogon_decider_guest_sdk::evolve_one::<
                        #canonical_ty,
                        __trogon_decider_bindings::DomainError,
                        __trogon_decider_bindings::AnyEnvelope,
                    >(state, envelope)?;
                }
                *self.state.borrow_mut() = state;
                Ok(())
            }

            fn decide(
                &self,
                command: __trogon_decider_bindings::CommandEnvelope,
            ) -> Result<
                Vec<__trogon_decider_bindings::AnyEnvelope>,
                __trogon_decider_bindings::DecideError,
            > {
                let envelope = command;
                match envelope.type_.as_str() {
                    #(#decide_arms,)*
                    other => Err(__trogon_decider_bindings::DecideError::Faulted(
                        __trogon_decider_bindings::DomainError {
                            code: "invalid-command".to_string(),
                            message: format!("unknown command type '{other}'"),
                        },
                    )),
                }
            }

            fn snapshot(&self) -> Option<Vec<u8>> {
                ::trogon_decider_guest_sdk::encode_current::<
                    <#canonical_ty as ::trogon_decider::Decider>::State,
                >(&self.state.borrow(), #state_schema_version)
            }
        }

        __trogon_decider_bindings::export!(Component with_types_in __trogon_decider_bindings);
    };

    expanded.into()
}

struct CommandSpec {
    ty: syn::Type,
    type_url: syn::Expr,
    proto: syn::Type,
    module: syn::LitStr,
    version: syn::LitStr,
    state_schema_version: syn::Expr,
    write_precondition: WritePreconditionSpec,
}

enum WritePreconditionSpec {
    Default,
    Any,
    StreamExists,
    NoStream,
}

impl syn::parse::Parse for CommandSpec {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let ty: syn::Type = input.parse()?;
        let content;
        syn::braced!(content in input);
        let mut type_url = None;
        let mut proto = None;
        let mut module = None;
        let mut version = None;
        let mut state_schema_version = None;
        let mut write_precondition = WritePreconditionSpec::Default;

        while !content.is_empty() {
            let key: syn::Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "type_url" => type_url = Some(content.parse()?),
                "proto" => proto = Some(content.parse()?),
                "module" => module = Some(content.parse()?),
                "version" => version = Some(content.parse()?),
                "state_schema_version" => state_schema_version = Some(content.parse()?),
                "write_precondition" => {
                    let value: syn::Ident = content.parse()?;
                    write_precondition = match value.to_string().as_str() {
                        "any" => WritePreconditionSpec::Any,
                        "stream_exists" => WritePreconditionSpec::StreamExists,
                        "no_stream" => WritePreconditionSpec::NoStream,
                        "default" => WritePreconditionSpec::Default,
                        other => {
                            return Err(syn::Error::new(
                                value.span(),
                                format!("unknown write_precondition '{other}'"),
                            ));
                        }
                    };
                }
                other => return Err(syn::Error::new(key.span(), format!("unknown field '{other}'"))),
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            ty,
            type_url: type_url.ok_or_else(|| syn::Error::new(input.span(), "missing type_url"))?,
            proto: proto.ok_or_else(|| syn::Error::new(input.span(), "missing proto"))?,
            module: module.ok_or_else(|| syn::Error::new(input.span(), "missing module"))?,
            version: version.ok_or_else(|| syn::Error::new(input.span(), "missing version"))?,
            state_schema_version: state_schema_version
                .ok_or_else(|| syn::Error::new(input.span(), "missing state_schema_version"))?,
            write_precondition,
        })
    }
}
