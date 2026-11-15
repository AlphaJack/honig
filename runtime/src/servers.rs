//! Server URLs: templates, variables and the base path of an operation.

/// A server variable: (name, default, allowed values). Empty allowed values mean any.
pub type ServerVar = (&'static str, &'static str, &'static [&'static str]);

/// Returns `path` under the operation's first server (variables at their defaults).
/// An absolute result bypasses the transport's base URL.
pub fn operation_path(servers: &[&str], vars: &[&[ServerVar]], path: String) -> String {
    let (Some(url), Some(vars)) = (servers.first(), vars.first()) else { return path };
    let base = server_url(url, vars, &[]).unwrap_or_else(|_| (*url).to_owned());
    format!("{}{path}", base.trim_end_matches('/'))
}

/// Expands a server URL template. `values` come first and then the default of each variable.
/// Errors: a value outside the `enum` of a variable or for an undeclared variable.
pub fn server_url(template: &str, vars: &[ServerVar], values: &[(&str, &str)]) -> Result<String, String> {
    if let Some((name, _)) = values.iter().find(|(n, _)| !vars.iter().any(|(v, ..)| v == n)) {
        return Err(format!("server variable `{name}` is not declared"));
    }
    let mut url = template.to_owned();
    for (name, default, allowed) in vars {
        let value = values.iter().find(|(n, _)| n == name).map_or(*default, |(_, v)| *v);
        if !allowed.is_empty() && !allowed.contains(&value) {
            return Err(format!("`{value}` is not an allowed value of server variable `{name}`"));
        }
        url = url.replace(&format!("{{{name}}}"), value);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VARS: &[ServerVar] = &[("region", "eu", &["eu", "us"]), ("port", "443", &[])];
    const URL: &str = "https://{region}.api.example:{port}/v1";

    #[test]
    fn variables_take_their_value_else_their_default() {
        assert_eq!(server_url(URL, VARS, &[]).as_deref(), Ok("https://eu.api.example:443/v1"));
        assert_eq!(
            server_url(URL, VARS, &[("region", "us"), ("port", "8443")]).as_deref(),
            Ok("https://us.api.example:8443/v1")
        );
    }

    #[test]
    fn a_value_outside_the_enum_or_an_undeclared_variable_is_an_error() {
        assert_eq!(
            server_url(URL, VARS, &[("region", "ap")]),
            Err("`ap` is not an allowed value of server variable `region`".to_owned())
        );
        assert_eq!(server_url(URL, VARS, &[("zone", "a")]), Err("server variable `zone` is not declared".to_owned()));
    }

    #[test]
    fn an_operation_path_goes_under_the_first_server() {
        assert_eq!(
            operation_path(&[URL, "https://other.example"], &[VARS, &[]], "/orders".to_owned()),
            "https://eu.api.example:443/v1/orders"
        );
        assert_eq!(
            operation_path(&["https://api.example/"], &[&[]], "/orders".to_owned()),
            "https://api.example/orders",
            "one `/` between"
        );
        assert_eq!(operation_path(&[], &[], "/orders".to_owned()), "/orders", "no server: the path alone");
    }
}
