use crate::{
  check_is_apub_id_valid,
  context::lemmy_context,
  generate_outbox_url,
  objects::{get_summary_from_string_or_source, ImageObject, Source},
};
use activitystreams::{
  actor::Endpoints,
  base::AnyBase,
  chrono::NaiveDateTime,
  object::{kind::ImageType, Tombstone},
  primitives::OneOrMany,
  unparsed::Unparsed,
};
use chrono::{DateTime, FixedOffset};
use lemmy_api_common::blocking;
use lemmy_apub_lib::{
  signatures::PublicKey,
  traits::{ActorType, ApubObject},
  values::MediaTypeMarkdown,
  verify::verify_domains_match,
};
use lemmy_db_schema::{
  naive_now,
  source::person::{Person as DbPerson, PersonForm},
};
use lemmy_utils::{
  utils::{check_slurs, check_slurs_opt, convert_datetime, markdown_to_html},
  LemmyError,
};
use lemmy_websocket::LemmyContext;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::ops::Deref;
use url::Url;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub enum UserTypes {
  Person,
  Service,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
  #[serde(rename = "@context")]
  context: OneOrMany<AnyBase>,
  #[serde(rename = "type")]
  kind: UserTypes,
  id: Url,
  /// username, set at account creation and can never be changed
  preferred_username: String,
  /// displayname (can be changed at any time)
  name: Option<String>,
  summary: Option<String>,
  source: Option<Source>,
  /// user avatar
  icon: Option<ImageObject>,
  /// user banner
  image: Option<ImageObject>,
  matrix_user_id: Option<String>,
  inbox: Url,
  /// mandatory field in activitypub, currently empty in lemmy
  outbox: Url,
  endpoints: Endpoints<Url>,
  public_key: PublicKey,
  published: Option<DateTime<FixedOffset>>,
  updated: Option<DateTime<FixedOffset>>,
  #[serde(flatten)]
  unparsed: Unparsed,
}

// TODO: can generate this with a derive macro
impl Person {
  pub(crate) fn id(&self, expected_domain: &Url) -> Result<&Url, LemmyError> {
    verify_domains_match(&self.id, expected_domain)?;
    Ok(&self.id)
  }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ApubPerson(DbPerson);

impl Deref for ApubPerson {
  type Target = DbPerson;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl From<DbPerson> for ApubPerson {
  fn from(p: DbPerson) -> Self {
    ApubPerson { 0: p }
  }
}

#[async_trait::async_trait(?Send)]
impl ApubObject for ApubPerson {
  type DataType = LemmyContext;
  type ApubType = Person;
  type TombstoneType = Tombstone;

  fn last_refreshed_at(&self) -> Option<NaiveDateTime> {
    Some(self.last_refreshed_at)
  }

  async fn read_from_apub_id(
    object_id: Url,
    context: &LemmyContext,
  ) -> Result<Option<Self>, LemmyError> {
    Ok(
      blocking(context.pool(), move |conn| {
        DbPerson::read_from_apub_id(conn, object_id)
      })
      .await??
      .map(Into::into),
    )
  }

  async fn delete(self, context: &LemmyContext) -> Result<(), LemmyError> {
    blocking(context.pool(), move |conn| {
      DbPerson::update_deleted(conn, self.id, true)
    })
    .await??;
    Ok(())
  }

  async fn to_apub(&self, _pool: &LemmyContext) -> Result<Person, LemmyError> {
    let kind = if self.bot_account {
      UserTypes::Service
    } else {
      UserTypes::Person
    };
    let source = self.bio.clone().map(|bio| Source {
      content: bio,
      media_type: MediaTypeMarkdown::Markdown,
    });
    let icon = self.avatar.clone().map(|url| ImageObject {
      kind: ImageType::Image,
      url: url.into(),
    });
    let image = self.banner.clone().map(|url| ImageObject {
      kind: ImageType::Image,
      url: url.into(),
    });

    let person = Person {
      context: lemmy_context(),
      kind,
      id: self.actor_id.to_owned().into_inner(),
      preferred_username: self.name.clone(),
      name: self.display_name.clone(),
      summary: self.bio.as_ref().map(|b| markdown_to_html(b)),
      source,
      icon,
      image,
      matrix_user_id: self.matrix_user_id.clone(),
      published: Some(convert_datetime(self.published)),
      outbox: generate_outbox_url(&self.actor_id)?.into(),
      endpoints: Endpoints {
        shared_inbox: self.shared_inbox_url.clone().map(|s| s.into()),
        ..Default::default()
      },
      public_key: self.get_public_key()?,
      updated: self.updated.map(convert_datetime),
      unparsed: Default::default(),
      inbox: self.inbox_url.clone().into(),
    };
    Ok(person)
  }

  fn to_tombstone(&self) -> Result<Tombstone, LemmyError> {
    unimplemented!()
  }

  async fn from_apub(
    person: &Person,
    context: &LemmyContext,
    expected_domain: &Url,
    _request_counter: &mut i32,
  ) -> Result<ApubPerson, LemmyError> {
    let actor_id = Some(person.id(expected_domain)?.clone().into());
    let name = person.preferred_username.clone();
    let display_name: Option<String> = person.name.clone();
    let bio = get_summary_from_string_or_source(&person.summary, &person.source);
    let shared_inbox = person.endpoints.shared_inbox.clone().map(|s| s.into());
    let bot_account = match person.kind {
      UserTypes::Person => false,
      UserTypes::Service => true,
    };

    let slur_regex = &context.settings().slur_regex();
    check_slurs(&name, slur_regex)?;
    check_slurs_opt(&display_name, slur_regex)?;
    check_slurs_opt(&bio, slur_regex)?;

    check_is_apub_id_valid(&person.id, false, &context.settings())?;

    let person_form = PersonForm {
      name,
      display_name: Some(display_name),
      banned: None,
      deleted: None,
      avatar: Some(person.icon.clone().map(|i| i.url.into())),
      banner: Some(person.image.clone().map(|i| i.url.into())),
      published: person.published.map(|u| u.clone().naive_local()),
      updated: person.updated.map(|u| u.clone().naive_local()),
      actor_id,
      bio: Some(bio),
      local: Some(false),
      admin: Some(false),
      bot_account: Some(bot_account),
      private_key: None,
      public_key: Some(Some(person.public_key.public_key_pem.clone())),
      last_refreshed_at: Some(naive_now()),
      inbox_url: Some(person.inbox.to_owned().into()),
      shared_inbox_url: Some(shared_inbox),
      matrix_user_id: Some(person.matrix_user_id.clone()),
    };
    let person = blocking(context.pool(), move |conn| {
      DbPerson::upsert(conn, &person_form)
    })
    .await??;
    Ok(person.into())
  }
}

impl ActorType for ApubPerson {
  fn is_local(&self) -> bool {
    self.local
  }
  fn actor_id(&self) -> Url {
    self.actor_id.to_owned().into_inner()
  }
  fn name(&self) -> String {
    self.name.clone()
  }

  fn public_key(&self) -> Option<String> {
    self.public_key.to_owned()
  }

  fn private_key(&self) -> Option<String> {
    self.private_key.to_owned()
  }

  fn inbox_url(&self) -> Url {
    self.inbox_url.clone().into()
  }

  fn shared_inbox_url(&self) -> Option<Url> {
    self.shared_inbox_url.clone().map(|s| s.into_inner())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::objects::tests::{file_to_json_object, init_context};
  use assert_json_diff::assert_json_include;
  use lemmy_db_schema::traits::Crud;
  use serial_test::serial;

  #[actix_rt::test]
  #[serial]
  async fn test_parse_lemmy_person() {
    let context = init_context();
    let json = file_to_json_object("assets/lemmy-person.json");
    let url = Url::parse("https://enterprise.lemmy.ml/u/picard").unwrap();
    let mut request_counter = 0;
    let person = ApubPerson::from_apub(&json, &context, &url, &mut request_counter)
      .await
      .unwrap();

    assert_eq!(person.actor_id.clone().into_inner(), url);
    assert_eq!(person.display_name, Some("Jean-Luc Picard".to_string()));
    assert!(person.public_key.is_some());
    assert!(!person.local);
    assert_eq!(person.bio.as_ref().unwrap().len(), 39);
    assert_eq!(request_counter, 0);

    let to_apub = person.to_apub(&context).await.unwrap();
    assert_json_include!(actual: json, expected: to_apub);

    DbPerson::delete(&*context.pool().get().unwrap(), person.id).unwrap();
  }

  #[actix_rt::test]
  #[serial]
  async fn test_parse_pleroma_person() {
    let context = init_context();
    let json = file_to_json_object("assets/pleroma-person.json");
    let url = Url::parse("https://queer.hacktivis.me/users/lanodan").unwrap();
    let mut request_counter = 0;
    let person = ApubPerson::from_apub(&json, &context, &url, &mut request_counter)
      .await
      .unwrap();

    assert_eq!(person.actor_id.clone().into_inner(), url);
    assert_eq!(person.name, "lanodan");
    assert!(person.public_key.is_some());
    assert!(!person.local);
    assert_eq!(request_counter, 0);
    assert_eq!(person.bio.as_ref().unwrap().len(), 873);

    DbPerson::delete(&*context.pool().get().unwrap(), person.id).unwrap();
  }
}
