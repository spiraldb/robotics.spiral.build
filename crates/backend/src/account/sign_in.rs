//! The browser's side of signing in. `/sign-in` sends the browser to Spiral's
//! sign-in page, which returns it to `/callback` with a code; Studio redeems
//! the code and names the new session in a cookie the browser sends with every
//! later request.

use std::time::Duration;

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{AppendHeaders, IntoResponse, Redirect, Response},
    routing::get,
};
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use spiral_robotics_common::account::SignInRequest;

use crate::projects::context::StudioContext;

/// How long a browser has to finish signing in on Spiral's page.
const SIGN_IN_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// How long a browser keeps its session cookie. Spiral decides how long the
/// session itself lasts.
const SESSION_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Where browsers reach this Studio, which Spiral's sign-in returns them to.
#[derive(Clone, Debug)]
pub(crate) struct PublicAddress {
    origin: String,
    secure: bool,
}

impl PublicAddress {
    /// An `http://` or `https://` origin, such as `http://localhost:3000`.
    pub(crate) fn parse(address: &str) -> Result<Self, String> {
        let url = url::Url::parse(address.trim())
            .map_err(|error| format!("Studio's public address {address:?}: {error}"))?;
        let origin = url.origin();
        if !matches!(url.scheme(), "http" | "https")
            || !origin.is_tuple()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(format!(
                "Studio's public address must be an http:// or https:// origin, not {address:?}"
            ));
        }
        Ok(Self {
            origin: origin.ascii_serialization(),
            secure: url.scheme() == "https",
        })
    }

    /// The page Spiral's sign-in returns the browser to. Spiral returns only
    /// to pages registered for its client.
    pub(crate) fn callback(&self) -> String {
        format!("{}{}", self.origin, SignInRequest::CALLBACK_PATH)
    }

    fn cookies(&self) -> BrowserCookies {
        BrowserCookies {
            secure: self.secure,
        }
    }
}

/// One browser's sign-in in progress. The state ties Spiral's answer to the
/// browser that asked, the verifier proves the code was issued to this
/// backend, and the browser returns to `return_to` once signed in.
pub(super) struct SignInAttempt {
    state: String,
    verifier: String,
    return_to: String,
}

impl SignInAttempt {
    pub(super) fn new(return_to: &str) -> Result<Self, String> {
        Ok(Self {
            state: Self::random()?,
            verifier: Self::random()?,
            return_to: return_to.to_owned(),
        })
    }

    pub(super) fn state(&self) -> &str {
        &self.state
    }

    pub(super) fn verifier(&self) -> &str {
        &self.verifier
    }

    /// The PKCE challenge Spiral's page receives in place of the verifier.
    pub(super) fn challenge(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(self.verifier.as_bytes()))
    }

    /// The attempt as the browser keeps it until the callback.
    fn cookie(&self) -> String {
        let return_to =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.return_to.as_bytes());
        format!("{}.{}.{return_to}", self.state, self.verifier)
    }

    fn from_cookie(cookie: &str) -> Option<Self> {
        let mut parts = cookie.split('.');
        let (state, verifier, return_to) = (parts.next()?, parts.next()?, parts.next()?);
        let return_to = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(return_to)
            .ok()?;
        Some(Self {
            state: state.to_owned(),
            verifier: verifier.to_owned(),
            return_to: SignInRequest::new(String::from_utf8(return_to).ok()?)
                .return_to()
                .to_owned(),
        })
    }

    fn random() -> Result<String, String> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|error| format!("no randomness: {error}"))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }
}

/// The cookies a browser keeps for Studio. Both are HttpOnly, so no page script
/// reads them, and SameSite=Lax, so no other site sends requests with them.
pub(crate) struct BrowserCookies {
    secure: bool,
}

impl BrowserCookies {
    /// The browser's session ID.
    pub(crate) const SESSION: &str = "studio_oauth_session";
    /// The browser's sign-in in progress.
    const SIGN_IN: &str = "studio_sign_in";

    pub(crate) fn read<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .filter_map(|cookie| cookie.trim().split_once('='))
            .find_map(|(key, value)| (key == name).then_some(value))
    }

    fn set(&self, name: &str, value: &str, lifetime: Duration) -> HeaderValue {
        let secure = if self.secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{name}={value}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax{secure}",
            lifetime.as_secs()
        ))
        .expect("cookie values are base64url")
    }

    pub(crate) fn clear(&self, name: &str) -> HeaderValue {
        self.set(name, "", Duration::ZERO)
    }
}

/// What Spiral's sign-in page returns: a code, or why there is none.
#[derive(Deserialize)]
struct SignInCallback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// The pages a browser signs in through. They are not part of the API: a
/// browser navigates to them.
pub(crate) struct BrowserSignIn;

impl BrowserSignIn {
    pub(crate) fn routes() -> Router<StudioContext> {
        Router::new()
            .route(SignInRequest::PATH, get(Self::start))
            .route(SignInRequest::CALLBACK_PATH, get(Self::finish))
    }

    /// The header that ends this browser's session cookie.
    pub(crate) fn signed_out(studio: &StudioContext) -> HeaderValue {
        studio
            .sessions()
            .address()
            .cookies()
            .clear(BrowserCookies::SESSION)
    }

    async fn start(
        State(studio): State<StudioContext>,
        Query(request): Query<SignInRequest>,
    ) -> Response {
        let attempt = match SignInAttempt::new(request.return_to()) {
            Ok(attempt) => attempt,
            Err(problem) => return Self::failed(&studio, &problem),
        };
        let cookies = studio.sessions().address().cookies();
        (
            AppendHeaders([(
                header::SET_COOKIE,
                cookies.set(BrowserCookies::SIGN_IN, &attempt.cookie(), SIGN_IN_LIFETIME),
            )]),
            Redirect::to(&studio.sessions().authorization_url(&attempt)),
        )
            .into_response()
    }

    async fn finish(
        State(studio): State<StudioContext>,
        headers: HeaderMap,
        Query(callback): Query<SignInCallback>,
    ) -> Response {
        if let Some(error) = &callback.error {
            let reason = callback.error_description.as_deref().unwrap_or(error);
            return Self::failed(&studio, &format!("Spiral did not sign you in: {reason}"));
        }
        let attempt = BrowserCookies::read(&headers, BrowserCookies::SIGN_IN)
            .and_then(SignInAttempt::from_cookie);
        let (Some(code), Some(state), Some(attempt)) = (callback.code, callback.state, attempt)
        else {
            return Self::failed(&studio, "This sign-in expired or began in another browser.");
        };
        if state != attempt.state {
            return Self::failed(&studio, "This sign-in expired or began in another browser.");
        }
        let session = match studio.sessions().complete(&code, &attempt).await {
            Ok(session) => session,
            Err(error) => return Self::failed(&studio, error.message()),
        };
        let cookies = studio.sessions().address().cookies();
        (
            AppendHeaders([
                (
                    header::SET_COOKIE,
                    cookies.set(BrowserCookies::SESSION, &session, SESSION_LIFETIME),
                ),
                (header::SET_COOKIE, cookies.clear(BrowserCookies::SIGN_IN)),
            ]),
            Redirect::to(&attempt.return_to),
        )
            .into_response()
    }

    /// A plain page saying why signing in stopped, and where to start over.
    fn failed(studio: &StudioContext, problem: &str) -> Response {
        let start_over = format!(
            "{}{}",
            studio.sessions().address().origin,
            SignInRequest::PATH
        );
        (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("{problem}\n\nTo try again, open {start_over}\n"),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_address_is_an_origin_whose_scheme_sets_cookie_security() {
        let local = PublicAddress::parse("http://localhost:3000").unwrap();
        assert_eq!(local.callback(), "http://localhost:3000/callback");
        assert!(
            !local
                .cookies()
                .set("a", "b", SESSION_LIFETIME)
                .to_str()
                .unwrap()
                .contains("Secure")
        );
        let hosted = PublicAddress::parse("https://robotics.spiral.build/").unwrap();
        assert_eq!(hosted.callback(), "https://robotics.spiral.build/callback");
        assert!(
            hosted
                .cookies()
                .clear("a")
                .to_str()
                .unwrap()
                .ends_with("; Secure")
        );
        for invalid in [
            "ftp://robotics.spiral.build",
            "https://robotics.spiral.build/studio",
            "https://robotics.spiral.build/?next=/",
            "robotics.spiral.build",
        ] {
            assert!(PublicAddress::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn an_attempt_survives_its_cookie_and_proves_its_challenge() {
        let attempt = SignInAttempt::new("/curation?draft=a.b").unwrap();
        let restored = SignInAttempt::from_cookie(&attempt.cookie()).unwrap();
        assert_eq!(restored.state(), attempt.state());
        assert_eq!(restored.verifier(), attempt.verifier());
        assert_eq!(restored.return_to, "/curation?draft=a.b");
        assert_ne!(attempt.state(), attempt.verifier());
        // RFC 7636's own example pair.
        let example = SignInAttempt {
            state: String::new(),
            verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
            return_to: "/".into(),
        };
        assert_eq!(
            example.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert!(SignInAttempt::from_cookie("only.two").is_none());
    }

    #[test]
    fn cookies_are_read_by_exact_name() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("other_studio_oauth_session=x; studio_oauth_session=abc"),
        );
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("studio_sign_in=s.v.r"),
        );
        assert_eq!(
            BrowserCookies::read(&headers, BrowserCookies::SESSION),
            Some("abc")
        );
        assert_eq!(
            BrowserCookies::read(&headers, BrowserCookies::SIGN_IN),
            Some("s.v.r")
        );
        assert_eq!(BrowserCookies::read(&headers, "missing"), None);
    }
}
