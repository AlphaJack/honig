//! Credentials and `OAuth` flows on the client: setters and token, device and authorization-code methods.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::emit::rust::http::shared::deprecated_scheme;
use crate::emit::rust::names::snake;
use crate::emit::rust::syntax::{doc_opt, lit};
use crate::ir::Api;
use crate::ir::names::Name;
use crate::ir::security::SchemeKind;

/// Returns the setter of the credential of a scheme. It is `with_<scheme>` and unique among the methods of the client.
pub(crate) fn setter_name(api: &Api, scheme: &str) -> proc_macro2::Ident {
    let mut taken = BTreeSet::new();
    for s in &api.schemes {
        let name = snake(&crate::ir::names::unique(Name::new(&format!("with_{}", s.name)), &mut taken));
        if s.name == scheme {
            return format_ident!("{name}");
        }
    }
    format_ident!("with_{}", snake(&Name::new(scheme)))
}

/// Returns one typed setter per security scheme. A misspelt scheme is a compile
/// error. Basic auth takes the user and password apart.
pub(crate) fn credential_setters(api: &Api) -> Vec<TokenStream> {
    api.schemes
        .iter()
        .filter_map(|s| {
            let (f, n) = (setter_name(api, &s.name), lit(&s.name));
            let dep = deprecated_scheme(s.deprecated);
            // The certificate belongs to the transport (its TLS configuration).
            if s.kind == SchemeKind::MutualTls {
                return None;
            }
            let doc = doc_opt(s.doc.as_deref());
            Some(if s.kind == SchemeKind::Basic {
                quote! {
                    #doc
                    #dep
                    #[must_use]
                    pub fn #f(mut self, user: &str, password: &str) -> Self {
                        self.credentials.insert(#n.to_owned(), format!("{user}:{password}"));
                        self
                    }
                }
            } else {
                quote! {
                    #doc
                    #dep
                    #[must_use]
                    pub fn #f(mut self, value: impl Into<String>) -> Self {
                        self.credentials.insert(#n.to_owned(), value.into());
                        self
                    }
                }
            })
        })
        .collect()
}

/// Returns `device_start_<scheme>` / `device_poll_<scheme>` for `OAuth2` device flows.
pub(crate) fn device_methods(api: &Api) -> Vec<TokenStream> {
    api.schemes.iter().filter_map(|s| {
        let (device_url, token_url) = s.device.as_ref()?;
        let snake = snake(&Name::new(&s.name));
        let (start, poll) = (format_ident!("device_start_{snake}"), format_ident!("device_poll_{snake}"));
        let (durl, turl, name) = (lit(device_url), lit(token_url), lit(&s.name));
        let dep = deprecated_scheme(s.deprecated);
        Some(quote! {
            #dep
            pub async fn #start(&self, client_id: &str, scopes: &[&str]) -> Result<runtime::DeviceCode, runtime::ClientError> {
                runtime::device_authorization(&self.transport, #durl, client_id, scopes).await
            }

            #dep
            pub async fn #poll(&mut self, client_id: &str, code: &runtime::DeviceCode) -> Result<runtime::DevicePoll, runtime::ClientError> {
                let poll = runtime::device_token(&self.transport, #turl, client_id, &code.device_code).await?;
                if let runtime::DevicePoll::Ready(token) = &poll {
                    self.credentials.insert(#name.to_owned(), token.access_token.clone());
                }
                Ok(poll)
            }
        })
    })
    .collect()
}

/// Returns password, authorization code (PKCE), implicit, refresh and discovery
/// helpers for `OAuth2` / `OpenID Connect` schemes by the flows that they declare.
pub(crate) fn oauth_methods(api: &Api) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut out = vec![];
    // URL builders need no transport: they go in the unbounded `impl`.
    let mut urls = vec![];
    for s in &api.schemes {
        let snake = snake(&Name::new(&s.name));
        let name = lit(&s.name);
        let dep = deprecated_scheme(s.deprecated);
        let store = quote!(self.credentials.insert(#name.to_owned(), token.access_token.clone()););
        if let Some(url) = &s.password_url {
            let (f, url) = (format_ident!("authorize_password_{snake}"), lit(url));
            out.push(quote! {
                #dep
                pub async fn #f(&mut self, username: &str, password: &str, client_id: &str, client_secret: Option<&str>, scopes: &[&str]) -> Result<runtime::Token, runtime::ClientError> {
                    let token = runtime::password_grant(&self.transport, #url, username, password, client_id, client_secret, scopes).await?;
                    #store
                    Ok(token)
                }
            });
        }
        if let Some((auth, token_url)) = &s.code {
            let (u, x) = (format_ident!("authorization_url_{snake}"), format_ident!("exchange_code_{snake}"));
            let (auth, token_url) = (lit(auth), lit(token_url));
            urls.push(quote! {
                #dep
                #[must_use]
                pub fn #u(client_id: &str, redirect_uri: &str, scopes: &[&str], state: &str, pkce: &runtime::Pkce) -> String {
                    runtime::authorization_url(#auth, client_id, redirect_uri, scopes, state, pkce)
                }
            });
            out.push(quote! {
                #dep
                pub async fn #x(&mut self, code: &str, redirect_uri: &str, client_id: &str, client_secret: Option<&str>, pkce: &runtime::Pkce) -> Result<runtime::Token, runtime::ClientError> {
                    let token = runtime::exchange_code(&self.transport, #token_url, code, redirect_uri, client_id, client_secret, pkce).await?;
                    #store
                    Ok(token)
                }
            });
        }
        if let Some(auth) = &s.implicit_url {
            let (f, auth) = (format_ident!("implicit_url_{snake}"), lit(auth));
            urls.push(quote! {
                #dep
                #[must_use]
                pub fn #f(client_id: &str, redirect_uri: &str, scopes: &[&str], state: &str) -> String {
                    runtime::implicit_url(#auth, client_id, redirect_uri, scopes, state)
                }
            });
        }
        if let Some(url) = s
            .refresh_url
            .as_ref()
            .filter(|_| s.code.is_some() || s.password_url.is_some() || s.device.is_some() || s.token_url.is_some())
        {
            let (f, url) = (format_ident!("refresh_{snake}"), lit(url));
            out.push(quote! {
                #dep
                pub async fn #f(&mut self, refresh_token: &str, client_id: &str, client_secret: Option<&str>) -> Result<runtime::Token, runtime::ClientError> {
                    let token = runtime::refresh_token(&self.transport, #url, refresh_token, client_id, client_secret).await?;
                    #store
                    Ok(token)
                }
            });
        }
        if let Some(url) = &s.discovery_url {
            let (f, url) = (format_ident!("discover_{snake}"), lit(url));
            out.push(quote! {
                #dep
                pub async fn #f(&self) -> Result<runtime::AuthMetadata, runtime::ClientError> {
                    runtime::discover(&self.transport, #url).await
                }
            });
        }
    }
    (out, urls)
}

/// Returns `authorize_<scheme>` methods for `OAuth2` client credentials flows.
pub(crate) fn token_methods(api: &Api) -> Vec<TokenStream> {
    // OAuth 2.0 client credentials: fetch a token and keep it as the credential.
    api.schemes.iter().filter_map(|s| {
        let url = lit(s.token_url.as_deref()?);
        let name = lit(&s.name);
        let f = format_ident!("authorize_{}", snake(&Name::new(&s.name)));
        let dep = deprecated_scheme(s.deprecated);
        Some(quote! {
            #dep
            pub async fn #f(mut self, client_id: &str, client_secret: &str, scopes: &[&str]) -> Result<Self, runtime::ClientError> {
                let token = runtime::client_credentials(&self.transport, #url, client_id, client_secret, scopes).await?;
                self.credentials.insert(#name.to_owned(), token.access_token);
                Ok(self)
            }
        })
    })
    .collect()
}
