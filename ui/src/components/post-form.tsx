import { Component, linkEvent } from 'inferno';
import { Subscription } from "rxjs";
import { retryWhen, delay, take } from 'rxjs/operators';
import { PostForm as PostFormI, Post, PostResponse, UserOperation, Community, ListCommunitiesResponse, ListCommunitiesForm, SortType } from '../interfaces';
import { WebSocketService, UserService } from '../services';
import { msgOp } from '../utils';
import * as autosize from 'autosize';

interface PostFormProps {
  post?: Post; // If a post is given, that means this is an edit
  prevCommunityName?: string;
  onCancel?(): any;
  onCreate?(id: number): any;
  onEdit?(post: Post): any;
}

interface PostFormState {
  postForm: PostFormI;
  communities: Array<Community>;
  loading: boolean;
}

export class PostForm extends Component<PostFormProps, PostFormState> {

  private subscription: Subscription;
  private emptyState: PostFormState = {
    postForm: {
      name: null,
      auth: null,
      community_id: null,
      creator_id: (UserService.Instance.user) ? UserService.Instance.user.id : null,
    },
    communities: [],
    loading: false
  }

  constructor(props: any, context: any) {
    super(props, context);

    this.state = this.emptyState;

    if (this.props.post) {
      this.state.postForm = {
        body: this.props.post.body,
        name: this.props.post.name,
        community_id: this.props.post.community_id,
        edit_id: this.props.post.id,
        creator_id: this.props.post.creator_id,
        url: this.props.post.url,
        auth: null
      }
    }

    this.subscription = WebSocketService.Instance.subject
      .pipe(retryWhen(errors => errors.pipe(delay(3000), take(10))))
      .subscribe(
        (msg) => this.parseMessage(msg),
        (err) => console.error(err),
        () => console.log('complete')
      );

      let listCommunitiesForm: ListCommunitiesForm = {
        sort: SortType[SortType.TopAll],
        limit: 9999,
      }

      WebSocketService.Instance.listCommunities(listCommunitiesForm);
  }

  componentDidMount() {
    autosize(document.querySelectorAll('textarea'));
  }

  componentWillUnmount() {
    this.subscription.unsubscribe();
  }

  render() {
    return (
      <div>
        <form onSubmit={linkEvent(this, this.handlePostSubmit)}>
          <div class="form-group row">
            <label class="col-sm-2 col-form-label">URL</label>
            <div class="col-sm-10">
              <input type="url" class="form-control" value={this.state.postForm.url} onInput={linkEvent(this, this.handlePostUrlChange)} />
            </div>
          </div>
          <div class="form-group row">
            <label class="col-sm-2 col-form-label">Title</label>
            <div class="col-sm-10">
              <textarea value={this.state.postForm.name} onInput={linkEvent(this, this.handlePostNameChange)} class="form-control" required rows={2} minLength={3} maxLength={100} />
            </div>
          </div>
          <div class="form-group row">
            <label class="col-sm-2 col-form-label">Body</label>
            <div class="col-sm-10">
              <textarea value={this.state.postForm.body} onInput={linkEvent(this, this.handlePostBodyChange)} class="form-control" rows={4} maxLength={10000} />
            </div>
          </div>
          {/* Cant change a community from an edit */}
          {!this.props.post &&
            <div class="form-group row">
            <label class="col-sm-2 col-form-label">Community</label>
            <div class="col-sm-10">
              <select class="form-control" value={this.state.postForm.community_id} onInput={linkEvent(this, this.handlePostCommunityChange)}>
                {this.state.communities.map(community =>
                  <option value={community.id}>{community.name}</option>
                )}
              </select>
            </div>
            </div>
            }
          <div class="form-group row">
            <div class="col-sm-10">
              <button type="submit" class="btn btn-secondary mr-2">
              {this.state.loading ? 
              <svg class="icon icon-spinner spin"><use xlinkHref="#icon-spinner"></use></svg> : 
              this.props.post ? 'Save' : 'Create'}</button>
              {this.props.post && <button type="button" class="btn btn-secondary" onClick={linkEvent(this, this.handleCancel)}>Cancel</button>}
            </div>
          </div>
        </form>
      </div>
    );
  }

  handlePostSubmit(i: PostForm, event: any) {
    event.preventDefault();
    if (i.props.post) {
      WebSocketService.Instance.editPost(i.state.postForm);
    } else {
      WebSocketService.Instance.createPost(i.state.postForm);
    }
    i.state.loading = true;
    i.setState(i.state);
  }

  handlePostUrlChange(i: PostForm, event: any) {
    i.state.postForm.url = event.target.value;
    i.setState(i.state);
  }

  handlePostNameChange(i: PostForm, event: any) {
    i.state.postForm.name = event.target.value;
    i.setState(i.state);
  }

  handlePostBodyChange(i: PostForm, event: any) {
    i.state.postForm.body = event.target.value;
    i.setState(i.state);
  }

  handlePostCommunityChange(i: PostForm, event: any) {
    i.state.postForm.community_id = Number(event.target.value);
    i.setState(i.state);
  }

  handleCancel(i: PostForm) {
    i.props.onCancel();
  }

  parseMessage(msg: any) {
    let op: UserOperation = msgOp(msg);
    if (msg.error) {
      alert(msg.error);
      this.state.loading = false;
      this.setState(this.state);
      return;
    } else if (op == UserOperation.ListCommunities) {
      let res: ListCommunitiesResponse = msg;
      this.state.communities = res.communities;
      if (this.props.post) {
        this.state.postForm.community_id = this.props.post.community_id;
      } else if (this.props.prevCommunityName) {
        let foundCommunityId = res.communities.find(r => r.name == this.props.prevCommunityName).id;
        this.state.postForm.community_id = foundCommunityId;
      } else {
        this.state.postForm.community_id = res.communities[0].id;
      }
      this.setState(this.state);
    } else if (op == UserOperation.CreatePost) {
      this.state.loading = false;
      let res: PostResponse = msg;
      this.props.onCreate(res.post.id);
    } else if (op == UserOperation.EditPost) {
      this.state.loading = false;
      let res: PostResponse = msg;
      this.props.onEdit(res.post);
    }
  }

}


