//! A person proving who they are to `jeden headless` with the Wisent identity
//! they already signed in with, instead of a client certificate the phone had
//! to be issued and keep.
//!
//! The daemon does not read the token itself. It asks Wisent Identity, the
//! authority every other Wisent server asks (Brama, Skrzynka, Most), whether
//! the bearer is a member of the organization the client names, and admits the
//! connection only when that answer names both.

use serde::Deserialize;
use serde_json::json;

pub const WISENT_ORGANIZATION_HEADER: &str = "x-wisent-organization-id";

/// A user Wisent Identity confirmed as a member of one organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WisentMember {
    pub user_id: uuid::Uuid,
    pub organization_id: uuid::Uuid,
    pub role: String,
}

/// Why a bearer was not admitted. Each variant is a different answer to the
/// person: sign in again, ask for membership, or retry later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityRefusal {
    /// The organization id is not a UUID.
    InvalidOrganization,
    /// Wisent Identity does not accept the bearer (expired or not a session).
    Unauthorized,
    /// The bearer is a real user who is not a member of that organization.
    Forbidden,
    /// Wisent Identity could not be asked or gave no usable answer.
    Unavailable(String),
}

impl IdentityRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidOrganization => "invalid_organization",
            Self::Unauthorized => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::Unavailable(_) => "identity_unavailable",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::InvalidOrganization => "organizationId must be a Wisent organization UUID".into(),
            Self::Unauthorized => "Wisent Identity rejected the access token; sign in again".into(),
            Self::Forbidden => "the signed-in user is not a member of that organization".into(),
            Self::Unavailable(cause) => format!("Wisent Identity could not be asked: {cause}"),
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

#[derive(Deserialize)]
struct OrganizationAuthorization {
    user_id: uuid::Uuid,
    organization_id: uuid::Uuid,
    role: String,
}

/// Where Wisent Identity answers: `JEDEN_WISENT_AUTH_URL` and
/// `JEDEN_WISENT_AUTH_ANON_KEY`, from the environment or `~/.jeden/.env`.
/// Nothing is compiled in: a daemon that names no authority refuses every
/// person-authenticated connection with the variable it lacks.
#[derive(Clone)]
pub struct WisentIdentityAuthority {
    client: reqwest::Client,
    origin: String,
    anon_key: String,
}

impl WisentIdentityAuthority {
    pub fn from_environment() -> Result<Self, String> {
        let origin = configured("JEDEN_WISENT_AUTH_URL")
            .ok_or_else(|| {
                "JEDEN_WISENT_AUTH_URL is not set; Wisent Identity has no address".to_string()
            })?
            .trim_end_matches('/')
            .to_owned();
        if !origin.starts_with("https://") {
            return Err(format!(
                "JEDEN_WISENT_AUTH_URL must be an https origin, got {origin}"
            ));
        }
        let anon_key = configured("JEDEN_WISENT_AUTH_ANON_KEY")
            .ok_or_else(|| "JEDEN_WISENT_AUTH_ANON_KEY is not set".to_string())?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| format!("Wisent Identity client could not be built: {error}"))?;
        Ok(Self {
            client,
            origin,
            anon_key,
        })
    }

    /// Ask whether `bearer` belongs to a member of `organization`.
    pub async fn authorize(
        &self,
        bearer: &str,
        organization: &str,
    ) -> Result<WisentMember, IdentityRefusal> {
        let organization_id = uuid::Uuid::parse_str(organization.trim())
            .map_err(|_| IdentityRefusal::InvalidOrganization)?;
        if bearer.trim().is_empty() {
            return Err(IdentityRefusal::Unauthorized);
        }
        let response = self
            .client
            .post(format!(
                "{}/rest/v1/rpc/authorize_organization",
                self.origin
            ))
            .header("apikey", &self.anon_key)
            .header("Accept", "application/vnd.pgrst.object+json")
            .header(WISENT_ORGANIZATION_HEADER, organization_id.to_string())
            .bearer_auth(bearer.trim())
            .json(&json!({ "target_org_id": organization_id }))
            .send()
            .await
            .map_err(|error| IdentityRefusal::Unavailable(error.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(IdentityRefusal::Unauthorized);
        }
        // PostgREST answers 406 when the object request matched no row: the
        // user is real and simply not a member.
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_ACCEPTABLE
        {
            return Err(IdentityRefusal::Forbidden);
        }
        if !status.is_success() {
            return Err(IdentityRefusal::Unavailable(format!("HTTP {status}")));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| IdentityRefusal::Unavailable(error.to_string()))?;
        let answer: OrganizationAuthorization = serde_json::from_slice(&bytes)
            .map_err(|error| IdentityRefusal::Unavailable(format!("unreadable answer: {error}")))?;
        if answer.organization_id != organization_id {
            return Err(IdentityRefusal::Forbidden);
        }
        Ok(WisentMember {
            user_id: answer.user_id,
            organization_id: answer.organization_id,
            role: answer.role,
        })
    }
}

fn configured(variable: &str) -> Option<String> {
    std::env::var(variable)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
