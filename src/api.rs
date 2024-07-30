use std::path::Path;
use std::process::exit;
use std::sync::Arc;

use anyhow::anyhow;
use futures::{SinkExt, StreamExt};
use itertools::Itertools;
use log::error;
use poem::{Error, handler, IntoResponse, Response};
use poem::error::{InternalServerError};
use poem::http::StatusCode;
use poem::web::{Data, Html, Json, Query};
use poem::web::websocket::{Message, WebSocket};
use serde::Deserialize;
use tera::{Context, ErrorKind, Tera};
use crate::adaptors::{ChannelTransferObject, get_receiver, HrmState};
use crate::ProgramData;

// Wrapper struct needed for Poem
#[derive(Deserialize)]
pub struct OptionalTemplateName<T> {
    pub name: Option<T>
}

/// index page
///
/// Contains some information about the server and some links.
#[handler]
pub async fn index(data: Data<&Arc<ProgramData>>) -> Result<Html<String>, Error> {
    let mut context = Context::new();
    context.insert("template_names", &data.tera.read().await.get_template_names().sorted().collect::<Vec<&str>>());
    Tera::one_off(include_str!("../included_templates/index.html.tera"), &context, true)
        .map_err(InternalServerError)
        .map(Html)
}

/// Lists all available [`tera::Tera`] templates as strings.
#[handler]
pub async fn list_templates(data: Data<&Arc<ProgramData>>) -> String {
    data.tera.read().await.get_template_names().sorted().join("\n")
}

/// Returns the actual HeartRate data as json.
#[handler]
pub async fn heart_rate(data: Data<&Arc<ProgramData>>) -> Json<ChannelTransferObject> {
    Json(data.0.hr_data.read().await.to_owned())
}

/// Renders a specific [`tera::Tera`] template, if existing.
#[handler]
pub async fn template(Query(OptionalTemplateName {name}): Query<OptionalTemplateName<String>>, data: Data<&Arc<ProgramData>>) -> Result<Html<String>, poem::Error> {
    let mut context = Context::new();
    if let Some(ref state) = data.0.hr_data.read().await.hr_state {
        match state {
            HrmState::Disconnected => context.insert("hr_disc", &true),
            HrmState::Ok(v) => {
                context.insert("hr_disc", &false);
                context.insert("hr_val", &v.hr);
                context.insert("hr_connected", &v.contact_ok);
                context.insert("hr_battery", &v.battery);
            }
        }
    }

    let template_name_value = name.unwrap_or("default.html".to_owned());

    // search for template, render it and return result or error
    data.0.tera.read().await
        .render(template_name_value.as_str(), &context)
        .map_err(|err| {
            if let ErrorKind::TemplateNotFound(v) = err.kind {
                Error::from_response(
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(format!("Template with name \"{v}\" was not found")))
            } else {
                error!("Error while rendering: {} gave error: {}", template_name_value, err);
                InternalServerError(err)
            }
        })
        .map(Html)
}

/// Reloads all [`tera::Tera`] templates.
///
/// If reloading returns an error:
/// - The current templates are kept.
/// - The client will be notified.
#[handler]
pub async fn reload_templates(data: Data<&Arc<ProgramData>>) -> Result<String, Error> {
    // get folder and perform basic checks
    let path = &data.merged_config.read().await.program_config.http_template_folder;
    if let Some(folder) = path {
        if !folder.exists() {
            return Err(Error::from_string(
                format!("Template folder \"{}\" does not exist!", folder.display()),
                StatusCode::INTERNAL_SERVER_ERROR)
            );
        }
        if !folder.is_dir() {
            return Err(Error::from_string(
                format!("Template folder \"{}\" is not a folder!", folder.display()),
                StatusCode::INTERNAL_SERVER_ERROR)
            );
        }
    }

    // load templates, this will return as new Tera instance
    match load_templates(path, false).await {
        Ok(tera) => {
            // store new instance for usage
            *data.tera.write().await = tera;
            Ok("Templates reloaded.".to_owned())
        }
        Err(err) => {
            Err(Error::from_string(
                format!("Template reload failed: {err}"),
                StatusCode::INTERNAL_SERVER_ERROR
            ))
        }
    }
}

/// Websocket endpoint
#[handler]
pub fn ws(
    ws: WebSocket
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        let (mut sink, _) = socket.split();

        tokio::spawn(async move {
            // every time we get a value from the HeartRate Manager, forward it to all clients
            let mut receiver = get_receiver();
            while let Ok(msg) = receiver.recv().await {
                if let Ok(data) = serde_json::to_string(&msg) {
                    if sink.send(Message::Text(data)).await.is_err() {
                        break;
                    }
                }
            }
        });
    })
}

/// Loads all templates.
///
/// Returns a [`tera::Tera`] instance with the templates.
/// Adds some default templates to the instance.
/// These default templates will not overwrite existing names.
pub async fn load_templates(http_template_folder: &Option<Box<Path>>, do_exit: bool) -> anyhow::Result<Tera> {
    let mut tera = match http_template_folder {
        // if we do not have a template folder, return empty instance
        None => Tera::default(),
        Some(p) => {
            // load and parse all templates in folder
            match Tera::new(
                format!("{}/**/*.{{html,html.tera,htm,htm.tera}}",
                        p
                            .to_str()
                            .ok_or(anyhow!("Invalid path for templates!"))?
                            .trim_end_matches('/')
                ).as_str()
            ) {
                Ok(t) => t,
                Err(e) => {
                    if do_exit {
                        error!("Parsing error(s): {e}");
                        exit(1);
                    } else {
                        return Err(anyhow!(e));
                    }
                }
            }
        }
    };

    // add some default templates, if no other templates with same name exist
    if !tera.get_template_names().any(|t| t == "default.html") {
        tera.add_raw_template("default.html", include_str!("../included_templates/default.html.tera"))?;
    }
    if !tera.get_template_names().any(|t| t == "ws.html") {
        tera.add_raw_template("ws.html", include_str!("../included_templates/ws.html.tera"))?;
    }
    Ok(tera)
}