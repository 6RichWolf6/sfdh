use crate::{
  activities::verify_person_in_community,
  context::lemmy_context,
  fetcher::object_id::ObjectId,
  migrations::CommentInReplyToMigration,
  objects::{create_tombstone, person::ApubPerson, post::ApubPost, Source},
  PostOrComment,
};
use activitystreams::{
  base::AnyBase,
  chrono::NaiveDateTime,
  object::{kind::NoteType, Tombstone},
  primitives::OneOrMany,
  unparsed::Unparsed,
};
use anyhow::{anyhow, Context};
use chrono::{DateTime, FixedOffset};
use lemmy_api_common::blocking;
use lemmy_apub_lib::{
  traits::{ApubObject, FromApub, ToApub},
  values::{MediaTypeHtml, MediaTypeMarkdown, PublicUrl},
  verify::verify_domains_match,
};
use lemmy_db_schema::{
  newtypes::CommentId,
  source::{
    comment::{Comment, CommentForm},
    community::Community,
    person::Person,
    post::Post,
  },
  traits::Crud,
  DbPool,
};
use lemmy_utils::{
  location_info,
  utils::{convert_datetime, remove_slurs},
  LemmyError,
};
use lemmy_websocket::LemmyContext;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::ops::Deref;
use url::Url;

#[skip_serializing_none]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
  #[serde(rename = "@context")]
  context: OneOrMany<AnyBase>,
  r#type: NoteType,
  id: Url,
  pub(crate) attributed_to: ObjectId<ApubPerson>,
  /// Indicates that the object is publicly readable. Unlike [`Post.to`], this one doesn't contain
  /// the community ID, as it would be incompatible with Pleroma (and we can get the community from
  /// the post in [`in_reply_to`]).
  to: PublicUrl,
  content: String,
  media_type: MediaTypeHtml,
  source: Source,
  in_reply_to: CommentInReplyToMigration,
  published: Option<DateTime<FixedOffset>>,
  updated: Option<DateTime<FixedOffset>>,
  #[serde(flatten)]
  unparsed: Unparsed,
}

impl Note {
  pub(crate) fn id_unchecked(&self) -> &Url {
    &self.id
  }
  pub(crate) fn id(&self, expected_domain: &Url) -> Result<&Url, LemmyError> {
    verify_domains_match(&self.id, expected_domain)?;
    Ok(&self.id)
  }

  pub(crate) async fn get_parents(
    &self,
    context: &LemmyContext,
    request_counter: &mut i32,
  ) -> Result<(ApubPost, Option<CommentId>), LemmyError> {
    dbg!(10);
    match &self.in_reply_to {
      CommentInReplyToMigration::Old(in_reply_to) => {
        dbg!(11);
        // This post, or the parent comment might not yet exist on this server yet, fetch them.
        let post_id = in_reply_to.get(0).context(location_info!())?;
        let post_id = ObjectId::new(post_id.clone());
        let post = Box::pin(post_id.dereference(context, request_counter)).await?;

        // The 2nd item, if it exists, is the parent comment apub_id
        // Nested comments will automatically get fetched recursively
        dbg!(12);
        let parent_id: Option<CommentId> = match in_reply_to.get(1) {
          Some(comment_id) => {
            let comment_id = ObjectId::<ApubComment>::new(comment_id.clone());
            let parent_comment = Box::pin(comment_id.dereference(context, request_counter)).await?;

            Some(parent_comment.id)
          }
          None => None,
        };
        dbg!(13);

        Ok((post, parent_id))
      }
      CommentInReplyToMigration::New(in_reply_to) => {
        dbg!(14);
        let parent = Box::pin(in_reply_to.dereference(context, request_counter).await?);
        match parent.deref() {
          PostOrComment::Post(p) => {
            dbg!(15);
            // Workaround because I cant figure out how to get the post out of the box (and we dont
            // want to stackoverflow in a deep comment hierarchy).
            let post_id = p.id;
            let post = blocking(context.pool(), move |conn| Post::read(conn, post_id)).await??;
            Ok((post.into(), None))
          }
          PostOrComment::Comment(c) => {
            dbg!(16);
            let post_id = c.post_id;
            let post = blocking(context.pool(), move |conn| Post::read(conn, post_id)).await??;
            Ok((post.into(), Some(c.id)))
          }
        }
      }
    }
  }

  pub(crate) async fn verify(
    &self,
    context: &LemmyContext,
    request_counter: &mut i32,
  ) -> Result<(), LemmyError> {
    let (post, _parent_comment_id) = self.get_parents(context, request_counter).await?;
    let community_id = post.community_id;
    let community = blocking(context.pool(), move |conn| {
      Community::read(conn, community_id)
    })
    .await??;

    if post.locked {
      return Err(anyhow!("Post is locked").into());
    }
    verify_domains_match(self.attributed_to.inner(), &self.id)?;
    verify_person_in_community(
      &self.attributed_to,
      &ObjectId::new(community.actor_id),
      context,
      request_counter,
    )
    .await?;
    Ok(())
  }
}

#[derive(Clone, Debug)]
pub struct ApubComment(Comment);

impl Deref for ApubComment {
  type Target = Comment;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl From<Comment> for ApubComment {
  fn from(c: Comment) -> Self {
    ApubComment { 0: c }
  }
}

#[async_trait::async_trait(?Send)]
impl ApubObject for ApubComment {
  type DataType = LemmyContext;

  fn last_refreshed_at(&self) -> Option<NaiveDateTime> {
    None
  }

  async fn read_from_apub_id(
    object_id: Url,
    context: &LemmyContext,
  ) -> Result<Option<Self>, LemmyError> {
    Ok(
      blocking(context.pool(), move |conn| {
        Comment::read_from_apub_id(conn, object_id)
      })
      .await??
      .map(Into::into),
    )
  }

  async fn delete(self, context: &LemmyContext) -> Result<(), LemmyError> {
    blocking(context.pool(), move |conn| {
      Comment::update_deleted(conn, self.id, true)
    })
    .await??;
    Ok(())
  }
}

#[async_trait::async_trait(?Send)]
impl ToApub for ApubComment {
  type ApubType = Note;
  type TombstoneType = Tombstone;
  type DataType = DbPool;

  async fn to_apub(&self, pool: &DbPool) -> Result<Note, LemmyError> {
    let creator_id = self.creator_id;
    let creator = blocking(pool, move |conn| Person::read(conn, creator_id)).await??;

    let post_id = self.post_id;
    let post = blocking(pool, move |conn| Post::read(conn, post_id)).await??;

    // Add a vector containing some important info to the "in_reply_to" field
    // [post_ap_id, Option(parent_comment_ap_id)]
    let mut in_reply_to_vec = vec![post.ap_id.into_inner()];

    if let Some(parent_id) = self.parent_id {
      let parent_comment = blocking(pool, move |conn| Comment::read(conn, parent_id)).await??;

      in_reply_to_vec.push(parent_comment.ap_id.into_inner());
    }

    let note = Note {
      context: lemmy_context(),
      r#type: NoteType::Note,
      id: self.ap_id.to_owned().into_inner(),
      attributed_to: ObjectId::new(creator.actor_id),
      to: PublicUrl::Public,
      content: self.content.clone(),
      media_type: MediaTypeHtml::Html,
      source: Source {
        content: self.content.clone(),
        media_type: MediaTypeMarkdown::Markdown,
      },
      in_reply_to: CommentInReplyToMigration::Old(in_reply_to_vec),
      published: Some(convert_datetime(self.published)),
      updated: self.updated.map(convert_datetime),
      unparsed: Default::default(),
    };

    Ok(note)
  }

  fn to_tombstone(&self) -> Result<Tombstone, LemmyError> {
    create_tombstone(
      self.deleted,
      self.ap_id.to_owned().into(),
      self.updated,
      NoteType::Note,
    )
  }
}

#[async_trait::async_trait(?Send)]
impl FromApub for ApubComment {
  type ApubType = Note;
  type DataType = LemmyContext;

  /// Converts a `Note` to `Comment`.
  ///
  /// If the parent community, post and comment(s) are not known locally, these are also fetched.
  async fn from_apub(
    note: &Note,
    context: &LemmyContext,
    expected_domain: &Url,
    request_counter: &mut i32,
  ) -> Result<ApubComment, LemmyError> {
    dbg!(1);
    let ap_id = Some(note.id(expected_domain)?.clone().into());
    let creator = note
      .attributed_to
      .dereference(context, request_counter)
      .await?;
    dbg!(2);
    let (post, parent_comment_id) = note.get_parents(context, request_counter).await?;
    dbg!(2.5);
    if post.locked {
      return Err(anyhow!("Post is locked").into());
    }

    let content = &note.source.content;
    let content_slurs_removed = remove_slurs(content, &context.settings().slur_regex());

    dbg!(3);
    let form = CommentForm {
      creator_id: creator.id,
      post_id: post.id,
      parent_id: parent_comment_id,
      content: content_slurs_removed,
      removed: None,
      read: None,
      published: note.published.map(|u| u.to_owned().naive_local()),
      updated: note.updated.map(|u| u.to_owned().naive_local()),
      deleted: None,
      ap_id,
      local: Some(false),
    };
    let comment = blocking(context.pool(), move |conn| Comment::upsert(conn, &form)).await??;
    dbg!(4);
    Ok(comment.into())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::objects::{
    community::ApubCommunity,
    tests::{file_to_json_object, init_context},
  };
  use serial_test::serial;

  async fn prepare_comment_test(url: &Url, context: &LemmyContext) {
    let person_json = file_to_json_object("assets/lemmy-person.json");
    ApubPerson::from_apub(&person_json, context, url, &mut 0)
      .await
      .unwrap();
    let community_json = file_to_json_object("assets/lemmy-community.json");
    ApubCommunity::from_apub(&community_json, context, url, &mut 0)
      .await
      .unwrap();
    let post_json = file_to_json_object("assets/lemmy-post.json");
    ApubPost::from_apub(&post_json, context, url, &mut 0)
      .await
      .unwrap();
  }

  #[actix_rt::test]
  #[serial]
  async fn test_fetch_lemmy_comment() {
    let context = init_context();
    let url = Url::parse("https://lemmy.ml/comment/38741").unwrap();
    prepare_comment_test(&url, &context).await;

    let json = file_to_json_object("assets/lemmy-comment.json");
    let mut request_counter = 0;
    let comment = ApubComment::from_apub(&json, &context, &url, &mut request_counter)
      .await
      .unwrap();

    assert_eq!(comment.ap_id.clone().into_inner(), url);
    assert_eq!(comment.content.len(), 1063);
    assert!(!comment.local);
    assert_eq!(request_counter, 0);
  }
}
