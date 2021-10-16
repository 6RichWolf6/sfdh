use crate::{
  activities::{
    generate_activity_id,
    private_message::delete::DeletePrivateMessage,
    verify_activity,
    verify_person,
  },
  context::lemmy_context,
  fetcher::object_id::ObjectId,
  send_lemmy_activity,
};
use activitystreams::{
  activity::kind::UndoType,
  base::AnyBase,
  primitives::OneOrMany,
  unparsed::Unparsed,
};
use lemmy_api_common::blocking;
use lemmy_apub_lib::{
  data::Data,
  traits::{ActivityFields, ActivityHandler, ActorType},
  verify::{verify_domains_match, verify_urls_match},
};
use lemmy_db_schema::{
  source::{person::Person, private_message::PrivateMessage},
  traits::Crud,
};
use lemmy_utils::LemmyError;
use lemmy_websocket::{send::send_pm_ws_message, LemmyContext, UserOperationCrud};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize, ActivityFields)]
#[serde(rename_all = "camelCase")]
pub struct UndoDeletePrivateMessage {
  actor: ObjectId<Person>,
  to: ObjectId<Person>,
  object: DeletePrivateMessage,
  #[serde(rename = "type")]
  kind: UndoType,
  id: Url,
  #[serde(rename = "@context")]
  context: OneOrMany<AnyBase>,
  #[serde(flatten)]
  unparsed: Unparsed,
}

impl UndoDeletePrivateMessage {
  pub async fn send(
    actor: &Person,
    pm: &PrivateMessage,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let recipient_id = pm.recipient_id;
    let recipient =
      blocking(context.pool(), move |conn| Person::read(conn, recipient_id)).await??;

    let object = DeletePrivateMessage::new(actor, pm, context)?;
    let id = generate_activity_id(
      UndoType::Undo,
      &context.settings().get_protocol_and_hostname(),
    )?;
    let undo = UndoDeletePrivateMessage {
      actor: ObjectId::new(actor.actor_id()),
      to: ObjectId::new(recipient.actor_id()),
      object,
      kind: UndoType::Undo,
      id: id.clone(),
      context: lemmy_context(),
      unparsed: Default::default(),
    };
    let inbox = vec![recipient.shared_inbox_or_inbox_url()];
    send_lemmy_activity(context, &undo, &id, actor, inbox, true).await
  }
}

#[async_trait::async_trait(?Send)]
impl ActivityHandler for UndoDeletePrivateMessage {
  type DataType = LemmyContext;
  async fn verify(
    &self,
    context: &Data<LemmyContext>,
    request_counter: &mut i32,
  ) -> Result<(), LemmyError> {
    verify_activity(self, &context.settings())?;
    verify_person(&self.actor, context, request_counter).await?;
    verify_urls_match(self.actor(), self.object.actor())?;
    verify_domains_match(self.actor(), self.object.object.inner())?;
    self.object.verify(context, request_counter).await?;
    Ok(())
  }

  async fn receive(
    self,
    context: &Data<LemmyContext>,
    _request_counter: &mut i32,
  ) -> Result<(), LemmyError> {
    let ap_id = self.object.object.clone();
    let private_message = ap_id.dereference_local(context).await?;

    let deleted_private_message = blocking(context.pool(), move |conn| {
      PrivateMessage::update_deleted(conn, private_message.id, false)
    })
    .await??;

    send_pm_ws_message(
      deleted_private_message.id,
      UserOperationCrud::EditPrivateMessage,
      None,
      context,
    )
    .await?;

    Ok(())
  }
}
