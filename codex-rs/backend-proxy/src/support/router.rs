use anyhow::Result;
use tiny_http::Method;

#[derive(Clone, Debug)]
pub struct Route {
    pub upstream_url: String,
}

#[derive(Clone, Debug)]
pub struct Router {
    base_url: String,
}

impl Router {
    pub fn new(base_url: &str) -> Result<Self> {
        let mut base = base_url.trim().to_string();
        while base.ends_with('/') {
            base.pop();
        }
        if base.is_empty() {
            return Err(anyhow::anyhow!("base_url must not be empty"));
        }
        Ok(Self { base_url: base })
    }

    pub fn match_route(&self, method: &Method, path: &str) -> Option<Route> {
        match (method, path) {
            (&Method::Post, "/v1/responses") => Some(Route {
                upstream_url: format!("{}/responses", self.base_url),
            }),
            _ => None,
        }
    }
}
