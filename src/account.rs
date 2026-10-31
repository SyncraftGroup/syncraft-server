use crate::config::Config;
use crate::error::Error;
use crate::log_error;
use crate::log_info;
use crate::message::{self, MessageKey};
use crate::paths::SyncraftPaths;
use crate::yggdrasil::db::DBManager;
use console_info::Ui;
use tabled::builder::Builder;
use tabled::settings::Style;

pub enum AccountAction {
    List,
    Add {
        email: String,
        character_name: String,
        password: String,
    },
    Remove {
        character_name: String,
    },
}

pub async fn run(paths: SyncraftPaths, action: AccountAction) -> Result<(), Error> {
    let config = match Config::load(&paths) {
        Some(c) => c,
        None => {
            Ui::get().error(&message::translate(&MessageKey::Uninitialized));
            return Ok(());
        }
    };

    if config.yggdrasil.base_url.is_empty() {
        Ui::get().error(&message::translate(
            &MessageKey::AccountYggdrasilNotConfigured,
        ));
        return Ok(());
    }

    let db = DBManager::init(&paths).await?;

    match action {
        AccountAction::List => cmd_list(&db).await?,
        AccountAction::Add {
            email,
            character_name,
            password,
        } => cmd_add(&db, &email, &character_name, &password).await?,
        AccountAction::Remove { character_name } => cmd_remove(&db, &character_name).await?,
    }

    Ok(())
}

async fn cmd_list(db: &DBManager) -> Result<(), Error> {
    let users = db.list_verified_users().await?;

    if users.is_empty() {
        Ui::get().echo(&message::translate(&MessageKey::AccountListEmpty));
        return Ok(());
    }

    let mut builder = Builder::new();
    builder.push_record(column_names());

    for user in &users {
        let has_skin = if user.skin_hash.is_some() { "Y" } else { "N" };
        builder.push_record([
            user.character_name.as_str(),
            user.email.as_str(),
            has_skin,
            user.created_at.as_str(),
        ]);
    }

    let mut table = builder.build();
    table.with(Style::modern());
    Ui::get().echo(&table.to_string());
    Ui::get().echo(&format!(
        "\n{}: {}",
        message::translate(&MessageKey::AccountTotal),
        users.len()
    ));

    Ok(())
}

fn column_names() -> Vec<String> {
    let name = message::translate(&MessageKey::AccountColName);
    let email = message::translate(&MessageKey::AccountColEmail);
    let skin = message::translate(&MessageKey::AccountColSkin);
    let created = message::translate(&MessageKey::AccountColCreated);
    vec![name, email, skin, created]
}

async fn cmd_add(
    db: &DBManager,
    email: &str,
    character_name: &str,
    password: &str,
) -> Result<(), Error> {
    if db.find_user_by_name(character_name).await?.is_some() {
        log_error!("Character '{}' already exists", character_name);
        Ui::get().error(&message::translate(&MessageKey::AccountCharExists {
            character_name: character_name.to_string(),
        }));
        return Ok(());
    }

    if db.find_user_by_email(email).await?.is_some() {
        log_error!("Email '{}' already exists", email);
        Ui::get().error(&message::translate(&MessageKey::AccountEmailExists {
            email: email.to_string(),
        }));
        return Ok(());
    }

    let user = db
        .create_user_verified(email, password, character_name)
        .await?;
    log_info!(
        "Account created: '{}' <{}> (id={})",
        user.character_name,
        user.email,
        user.id
    );

    Ui::get().ok(&message::translate(&MessageKey::AccountAdded {
        character_name: user.character_name,
    }));

    Ok(())
}

async fn cmd_remove(db: &DBManager, character_name: &str) -> Result<(), Error> {
    let removed = db.remove_user_by_name(character_name).await?;

    match removed {
        Some(user) => {
            log_info!(
                "Account removed: '{}' (id={})",
                user.character_name,
                user.id
            );
            Ui::get().ok(&message::translate(&MessageKey::AccountRemoved {
                character_name: user.character_name,
            }));
        }
        None => {
            log_error!("Character '{}' not found", character_name);
            Ui::get().error(&message::translate(&MessageKey::AccountCharNotFound {
                character_name: character_name.to_string(),
            }));
        }
    }

    Ok(())
}
