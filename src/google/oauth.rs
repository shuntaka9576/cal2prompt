use oauth2::{
    basic::{
        BasicClient, BasicErrorResponse, BasicRevocationErrorResponse,
        BasicTokenIntrospectionResponse, BasicTokenResponse,
    },
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    PkceCodeChallenge, RedirectUrl, RevocationUrl, Scope, StandardRevocableToken, TokenResponse,
    TokenUrl,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use webbrowser;

pub struct OAuth2Client {
    client: oauth2::Client<
        BasicErrorResponse,
        BasicTokenResponse,
        BasicTokenIntrospectionResponse,
        StandardRevocableToken,
        BasicRevocationErrorResponse,
        EndpointSet,    // Auth URL
        EndpointNotSet, // Device auth
        EndpointNotSet, // Introspection (not used)
        EndpointSet,    // Revocation (not used)
        EndpointSet,    // Token URL
    >,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
}

impl Token {
    pub fn from_token_response(response: &BasicTokenResponse) -> Self {
        let expires_at = response.expires_in().map(|duration| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default();
            (now + duration).as_secs() as i64
        });

        Token {
            access_token: response.access_token().secret().clone(),
            refresh_token: response.refresh_token().map(|r| r.secret().clone()),
            expires_at,
        }
    }

    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            exp <= now
        } else {
            false
        }
    }
}

impl OAuth2Client {
    pub fn new(client_id: &str, client_secret: &str, redirect_url: &str) -> Self {
        Self {
            client: BasicClient::new(ClientId::new(client_id.to_string()))
                .set_client_secret(ClientSecret::new(client_secret.to_string()))
                .set_auth_uri(
                    AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())
                        .expect("Invalid authorization endpoint URL"),
                )
                .set_token_uri(
                    TokenUrl::new("https://www.googleapis.com/oauth2/v3/token".to_string())
                        .expect("Invalid token endpoint URL"),
                )
                .set_redirect_uri(
                    RedirectUrl::new(redirect_url.to_string()).expect("Invalid redirect URL"),
                )
                .set_revocation_url(
                    RevocationUrl::new("https://oauth2.googleapis.com/revoke".to_string())
                        .expect("Invalid revocation endpoint URL"),
                ),
        }
    }

    pub async fn oauth_flow(&self) -> anyhow::Result<Token> {
        let http_client = reqwest::Client::new();

        let (pkce_code_challenge, pkce_code_verifier) = PkceCodeChallenge::new_random_sha256();

        let (authorize_url, _csrf_state) = self
            .client
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new(
                "https://www.googleapis.com/auth/calendar.readonly".to_string(),
            ))
            .set_pkce_challenge(pkce_code_challenge)
            .url();

        let redirect_url = self.client.redirect_uri().unwrap().to_string();
        let redirect_url_host = redirect_url
            .strip_prefix("http://")
            .unwrap_or(&redirect_url);

        let listener = tokio::net::TcpListener::bind(redirect_url_host).await?;
        webbrowser::open(authorize_url.as_ref()).unwrap();

        let (mut stream, _) = listener.accept().await?;

        let mut reader = AsyncBufReader::new(&mut stream);
        let mut redirect_request_line = String::new();

        reader.read_line(&mut redirect_request_line).await?;

        let redirect_url_and_path = Url::parse(
            &("http://localhost".to_string()
                + redirect_request_line.split_whitespace().nth(1).unwrap()),
        )?;

        let code = redirect_url_and_path
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, code)| AuthorizationCode::new(code.into_owned()))
            .ok_or(anyhow::anyhow!("no code"))?;

        let message = "Go back to your terminal :)";
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}",
            message.len(),
            message
        );
        stream.write_all(response.as_bytes()).await?;

        let token_response = self
            .client
            .exchange_code(code)
            .set_pkce_verifier(pkce_code_verifier)
            .request_async(&http_client)
            .await?;

        Ok(Token::from_token_response(&token_response))
    }

    pub async fn refresh_token(&self, refresh_token: String) -> anyhow::Result<Token> {
        let refresh_token = oauth2::RefreshToken::new(refresh_token);
        let http_client = reqwest::Client::new();
        let token_response = self
            .client
            .exchange_refresh_token(&refresh_token)
            .request_async(&http_client)
            .await?;

        Ok(Token::from_token_response(&token_response))
    }
}
