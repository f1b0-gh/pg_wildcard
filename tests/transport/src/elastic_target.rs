// SPDX-License-Identifier: Apache-2.0
//! Host-owned fixture destination and projection, independent of query commands.
use super::{Error, Result};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::{Client, RequestBuilder, Url};
use serde_json::{Value, json};

pub(super) struct Target {
    url: Url,
    authorization: Option<HeaderValue>,
    value_field: &'static str,
}
impl Target {
    pub(super) fn new(
        url: &str,
        auth: Option<&str>,
        index: Option<&str>,
        field: Option<&str>,
    ) -> Result<Self> {
        let mut url = Url::parse(url).map_err(|_| Error::Configuration)?;
        if url.scheme() != "http"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(Error::Configuration);
        }
        let index = index.unwrap_or("bench");
        if index.len() > 64
            || !(index == "bench" || index.starts_with("bench_"))
            || !index
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(Error::Configuration);
        }
        let value_field = match field.unwrap_or("v") {
            "v" => "v",
            "message" => "message",
            _ => return Err(Error::Configuration),
        };
        let authorization = auth
            .map(|value| {
                let encoded = value.strip_prefix("Basic ").ok_or(Error::Configuration)?;
                if encoded.is_empty()
                    || encoded.len() > 4096
                    || !encoded
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
                {
                    return Err(Error::Configuration);
                }
                let mut header = HeaderValue::from_str(value).map_err(|_| Error::Configuration)?;
                header.set_sensitive(true);
                Ok(header)
            })
            .transpose()?;
        url.set_path(&format!("/{index}/_search"));
        url.query_pairs_mut()
            .append_pair("request_cache", "false")
            .append_pair("allow_partial_search_results", "false");
        Ok(Self {
            url,
            authorization,
            value_field,
        })
    }
    pub(super) fn value_field(&self) -> &str {
        self.value_field
    }
    pub(super) fn authentication(&self) -> &str {
        if self.authorization.is_some() {
            "basic"
        } else {
            "none-diagnostic"
        }
    }
    pub(super) fn request(
        &self,
        client: &Client,
        body: &Value,
        mode: &str,
    ) -> Result<RequestBuilder> {
        let mut body = body.as_object().ok_or(Error::Input)?.clone();
        body.insert(
            "_source".into(),
            if mode == "count" {
                json!(false)
            } else {
                json!(["id", self.value_field])
            },
        );
        let mut request = client
            .post(self.url.clone())
            .header("content-type", "application/json")
            .body(Value::Object(body).to_string());
        if let Some(auth) = &self.authorization {
            request = request.header(AUTHORIZATION, auth.clone());
        }
        Ok(request)
    }
}

pub(super) fn optional_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(Error::Configuration),
    }
}
