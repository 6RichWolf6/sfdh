use crate::objects::{
  comment::{ApubComment, Note},
  post::{ApubPost, Page},
};
use activitystreams::chrono::NaiveDateTime;
use lemmy_apub_lib::traits::ApubObject;
use lemmy_db_schema::source::{comment::CommentForm, post::PostForm};
use lemmy_utils::LemmyError;
use lemmy_websocket::LemmyContext;
use serde::Deserialize;
use url::Url;

#[derive(Clone, Debug)]
pub enum PostOrComment {
  Post(Box<ApubPost>),
  Comment(ApubComment),
}

pub enum PostOrCommentForm {
  PostForm(Box<PostForm>),
  CommentForm(CommentForm),
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum PageOrNote {
  Page(Box<Page>),
  Note(Box<Note>),
}

#[async_trait::async_trait(?Send)]
impl ApubObject for PostOrComment {
  type DataType = LemmyContext;
  type ApubType = PageOrNote;
  type TombstoneType = ();

  fn last_refreshed_at(&self) -> Option<NaiveDateTime> {
    None
  }

  // TODO: this can probably be implemented using a single sql query
  async fn read_from_apub_id(
    object_id: Url,
    data: &Self::DataType,
  ) -> Result<Option<Self>, LemmyError>
  where
    Self: Sized,
  {
    let post = ApubPost::read_from_apub_id(object_id.clone(), data).await?;
    Ok(match post {
      Some(o) => Some(PostOrComment::Post(Box::new(o))),
      None => ApubComment::read_from_apub_id(object_id, data)
        .await?
        .map(PostOrComment::Comment),
    })
  }

  async fn delete(self, data: &Self::DataType) -> Result<(), LemmyError> {
    match self {
      PostOrComment::Post(p) => p.delete(data).await,
      PostOrComment::Comment(c) => c.delete(data).await,
    }
  }

  async fn to_apub(&self, _data: &Self::DataType) -> Result<Self::ApubType, LemmyError> {
    unimplemented!()
  }

  fn to_tombstone(&self) -> Result<Self::TombstoneType, LemmyError> {
    unimplemented!()
  }

  async fn from_apub(
    apub: &PageOrNote,
    context: &LemmyContext,
    expected_domain: &Url,
    request_counter: &mut i32,
  ) -> Result<Self, LemmyError>
  where
    Self: Sized,
  {
    Ok(match apub {
      PageOrNote::Page(p) => PostOrComment::Post(Box::new(
        ApubPost::from_apub(p, context, expected_domain, request_counter).await?,
      )),
      PageOrNote::Note(n) => PostOrComment::Comment(
        ApubComment::from_apub(n, context, expected_domain, request_counter).await?,
      ),
    })
  }
}

impl PostOrComment {
  pub(crate) fn ap_id(&self) -> Url {
    match self {
      PostOrComment::Post(p) => p.ap_id.clone(),
      PostOrComment::Comment(c) => c.ap_id.clone(),
    }
    .into()
  }
}
