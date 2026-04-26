const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobEvent_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobEvent {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobEvent>
}

impl ::protobuf::Message for JobEvent {}

impl ::std::default::Default for JobEvent {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobEvent {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobEvent` is `Sync` because it does not implement interior mutability.
//    Neither does `JobEventMut`.
unsafe impl Sync for JobEvent {}

// SAFETY:
// - `JobEvent` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobEvent {}

impl ::protobuf::Proxied for JobEvent {
  type View<'msg> = JobEventView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobEvent {}

impl ::protobuf::MutProxied for JobEvent {
  type Mut<'msg> = JobEventMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobEventView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobEvent>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobEventView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobEventView<'msg> {
  type Message = JobEvent;
}

impl ::std::fmt::Debug for JobEventView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobEventView<'_> {
  fn default() -> JobEventView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobEvent>> for JobEventView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobEvent>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobEventView<'msg> {

  pub fn to_owned(&self) -> JobEvent {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // job_added: optional message trogon.cron.jobs.v1.JobAdded
  pub fn has_job_added(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn job_added_opt(self) -> ::protobuf::Optional<super::JobAddedView<'msg>> {
        ::protobuf::Optional::new(self.job_added(), self.has_job_added())
  }
  pub fn job_added(self) -> super::JobAddedView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobAddedView::default())
  }

  // job_paused: optional message trogon.cron.jobs.v1.JobPaused
  pub fn has_job_paused(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn job_paused_opt(self) -> ::protobuf::Optional<super::JobPausedView<'msg>> {
        ::protobuf::Optional::new(self.job_paused(), self.has_job_paused())
  }
  pub fn job_paused(self) -> super::JobPausedView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobPausedView::default())
  }

  // job_resumed: optional message trogon.cron.jobs.v1.JobResumed
  pub fn has_job_resumed(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn job_resumed_opt(self) -> ::protobuf::Optional<super::JobResumedView<'msg>> {
        ::protobuf::Optional::new(self.job_resumed(), self.has_job_resumed())
  }
  pub fn job_resumed(self) -> super::JobResumedView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobResumedView::default())
  }

  // job_removed: optional message trogon.cron.jobs.v1.JobRemoved
  pub fn has_job_removed(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(3)
    }
  }
  pub fn job_removed_opt(self) -> ::protobuf::Optional<super::JobRemovedView<'msg>> {
        ::protobuf::Optional::new(self.job_removed(), self.has_job_removed())
  }
  pub fn job_removed(self) -> super::JobRemovedView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(3)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobRemovedView::default())
  }

  pub fn event(self) -> super::job_event::EventOneof<'msg> {
    match self.event_case() {
      super::job_event::EventCase::JobAdded =>
          super::job_event::EventOneof::JobAdded(self.job_added()),
      super::job_event::EventCase::JobPaused =>
          super::job_event::EventOneof::JobPaused(self.job_paused()),
      super::job_event::EventCase::JobResumed =>
          super::job_event::EventOneof::JobResumed(self.job_resumed()),
      super::job_event::EventCase::JobRemoved =>
          super::job_event::EventOneof::JobRemoved(self.job_removed()),
      _ => super::job_event::EventOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn event_case(self) -> super::job_event::EventCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_event::EventCase::try_from(field_num).unwrap_unchecked()
    }
  }
}

// SAFETY:
// - `JobEventView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobEventView<'_> {}

// SAFETY:
// - `JobEventView` is `Send` because while its alive a `JobEventMut` cannot.
// - `JobEventView` does not use thread-local data.
unsafe impl Send for JobEventView<'_> {}

impl<'msg> ::protobuf::AsView for JobEventView<'msg> {
  type Proxied = JobEvent;
  fn as_view(&self) -> ::protobuf::View<'msg, JobEvent> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobEventView<'msg> {
  fn into_view<'shorter>(self) -> JobEventView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobEvent> for JobEventView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobEvent {
    let mut dst = JobEvent::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobEvent> for JobEventMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobEvent {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobEvent {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobEventView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobEventMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobEventMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobEvent>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobEventMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobEventMut<'msg> {
  type Message = JobEvent;
}

impl ::std::fmt::Debug for JobEventMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobEvent>> for JobEventMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobEvent>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobEventMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobEvent> {
    self.inner
  }

  pub fn to_owned(&self) -> JobEvent {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // job_added: optional message trogon.cron.jobs.v1.JobAdded
  pub fn has_job_added(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_job_added(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn job_added_opt(&self) -> ::protobuf::Optional<super::JobAddedView<'_>> {
        ::protobuf::Optional::new(self.job_added(), self.has_job_added())
  }
  pub fn job_added(&self) -> super::JobAddedView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobAddedView::default())
  }
  pub fn job_added_mut(&mut self) -> super::JobAddedMut<'_> {
     let ptr = unsafe {
       self.inner.ptr_mut().get_or_create_mutable_message_at_index(
         0, self.inner.arena()
       ).unwrap()
     };
     ::protobuf::__internal::runtime::MessageMutInner::from_parent(
         self.as_message_mut_inner(::protobuf::__internal::Private),
         ptr
     ).into()
  }
  pub fn set_job_added(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobAdded>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val
      );
    }
  }

  // job_paused: optional message trogon.cron.jobs.v1.JobPaused
  pub fn has_job_paused(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_job_paused(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn job_paused_opt(&self) -> ::protobuf::Optional<super::JobPausedView<'_>> {
        ::protobuf::Optional::new(self.job_paused(), self.has_job_paused())
  }
  pub fn job_paused(&self) -> super::JobPausedView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobPausedView::default())
  }
  pub fn job_paused_mut(&mut self) -> super::JobPausedMut<'_> {
     let ptr = unsafe {
       self.inner.ptr_mut().get_or_create_mutable_message_at_index(
         1, self.inner.arena()
       ).unwrap()
     };
     ::protobuf::__internal::runtime::MessageMutInner::from_parent(
         self.as_message_mut_inner(::protobuf::__internal::Private),
         ptr
     ).into()
  }
  pub fn set_job_paused(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobPaused>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val
      );
    }
  }

  // job_resumed: optional message trogon.cron.jobs.v1.JobResumed
  pub fn has_job_resumed(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn clear_job_resumed(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        2
      );
    }
  }
  pub fn job_resumed_opt(&self) -> ::protobuf::Optional<super::JobResumedView<'_>> {
        ::protobuf::Optional::new(self.job_resumed(), self.has_job_resumed())
  }
  pub fn job_resumed(&self) -> super::JobResumedView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobResumedView::default())
  }
  pub fn job_resumed_mut(&mut self) -> super::JobResumedMut<'_> {
     let ptr = unsafe {
       self.inner.ptr_mut().get_or_create_mutable_message_at_index(
         2, self.inner.arena()
       ).unwrap()
     };
     ::protobuf::__internal::runtime::MessageMutInner::from_parent(
         self.as_message_mut_inner(::protobuf::__internal::Private),
         ptr
     ).into()
  }
  pub fn set_job_resumed(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobResumed>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        2,
        val
      );
    }
  }

  // job_removed: optional message trogon.cron.jobs.v1.JobRemoved
  pub fn has_job_removed(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(3)
    }
  }
  pub fn clear_job_removed(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        3
      );
    }
  }
  pub fn job_removed_opt(&self) -> ::protobuf::Optional<super::JobRemovedView<'_>> {
        ::protobuf::Optional::new(self.job_removed(), self.has_job_removed())
  }
  pub fn job_removed(&self) -> super::JobRemovedView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(3)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobRemovedView::default())
  }
  pub fn job_removed_mut(&mut self) -> super::JobRemovedMut<'_> {
     let ptr = unsafe {
       self.inner.ptr_mut().get_or_create_mutable_message_at_index(
         3, self.inner.arena()
       ).unwrap()
     };
     ::protobuf::__internal::runtime::MessageMutInner::from_parent(
         self.as_message_mut_inner(::protobuf::__internal::Private),
         ptr
     ).into()
  }
  pub fn set_job_removed(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobRemoved>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        3,
        val
      );
    }
  }

  pub fn event(&self) -> super::job_event::EventOneof<'_> {
    match &self.event_case() {
      super::job_event::EventCase::JobAdded =>
          super::job_event::EventOneof::JobAdded(self.job_added()),
      super::job_event::EventCase::JobPaused =>
          super::job_event::EventOneof::JobPaused(self.job_paused()),
      super::job_event::EventCase::JobResumed =>
          super::job_event::EventOneof::JobResumed(self.job_resumed()),
      super::job_event::EventCase::JobRemoved =>
          super::job_event::EventOneof::JobRemoved(self.job_removed()),
      _ => super::job_event::EventOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn event_case(&self) -> super::job_event::EventCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_event::EventCase::try_from(field_num).unwrap_unchecked()
    }
  }
}

// SAFETY:
// - `JobEventMut` does not perform any shared mutation.
unsafe impl Send for JobEventMut<'_> {}

// SAFETY:
// - `JobEventMut` does not perform any shared mutation.
unsafe impl Sync for JobEventMut<'_> {}

impl<'msg> ::protobuf::AsView for JobEventMut<'msg> {
  type Proxied = JobEvent;
  fn as_view(&self) -> ::protobuf::View<'_, JobEvent> {
    JobEventView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobEventMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobEvent>
  where
      'msg: 'shorter {
    JobEventView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobEventMut<'msg> {
  type MutProxied = JobEvent;
  fn as_mut(&mut self) -> JobEventMut<'msg> {
    JobEventMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobEventMut<'msg> {
  fn into_mut<'shorter>(self) -> JobEventMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobEvent {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobEvent> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobEventView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobEventMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // job_added: optional message trogon.cron.jobs.v1.JobAdded
  pub fn has_job_added(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_job_added(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn job_added_opt(&self) -> ::protobuf::Optional<super::JobAddedView<'_>> {
        ::protobuf::Optional::new(self.job_added(), self.has_job_added())
  }
  pub fn job_added(&self) -> super::JobAddedView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobAddedView::default())
  }
  pub fn job_added_mut(&mut self) -> super::JobAddedMut<'_> {
     let ptr = unsafe {
       self.inner.ptr_mut().get_or_create_mutable_message_at_index(
         0, self.inner.arena()
       ).unwrap()
     };
     ::protobuf::__internal::runtime::MessageMutInner::from_parent(
         self.as_message_mut_inner(::protobuf::__internal::Private),
         ptr
     ).into()
  }
  pub fn set_job_added(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobAdded>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val
      );
    }
  }

  // job_paused: optional message trogon.cron.jobs.v1.JobPaused
  pub fn has_job_paused(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_job_paused(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn job_paused_opt(&self) -> ::protobuf::Optional<super::JobPausedView<'_>> {
        ::protobuf::Optional::new(self.job_paused(), self.has_job_paused())
  }
  pub fn job_paused(&self) -> super::JobPausedView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobPausedView::default())
  }
  pub fn job_paused_mut(&mut self) -> super::JobPausedMut<'_> {
     let ptr = unsafe {
       self.inner.ptr_mut().get_or_create_mutable_message_at_index(
         1, self.inner.arena()
       ).unwrap()
     };
     ::protobuf::__internal::runtime::MessageMutInner::from_parent(
         self.as_message_mut_inner(::protobuf::__internal::Private),
         ptr
     ).into()
  }
  pub fn set_job_paused(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobPaused>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val
      );
    }
  }

  // job_resumed: optional message trogon.cron.jobs.v1.JobResumed
  pub fn has_job_resumed(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn clear_job_resumed(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        2
      );
    }
  }
  pub fn job_resumed_opt(&self) -> ::protobuf::Optional<super::JobResumedView<'_>> {
        ::protobuf::Optional::new(self.job_resumed(), self.has_job_resumed())
  }
  pub fn job_resumed(&self) -> super::JobResumedView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobResumedView::default())
  }
  pub fn job_resumed_mut(&mut self) -> super::JobResumedMut<'_> {
     let ptr = unsafe {
       self.inner.ptr_mut().get_or_create_mutable_message_at_index(
         2, self.inner.arena()
       ).unwrap()
     };
     ::protobuf::__internal::runtime::MessageMutInner::from_parent(
         self.as_message_mut_inner(::protobuf::__internal::Private),
         ptr
     ).into()
  }
  pub fn set_job_resumed(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobResumed>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        2,
        val
      );
    }
  }

  // job_removed: optional message trogon.cron.jobs.v1.JobRemoved
  pub fn has_job_removed(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(3)
    }
  }
  pub fn clear_job_removed(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        3
      );
    }
  }
  pub fn job_removed_opt(&self) -> ::protobuf::Optional<super::JobRemovedView<'_>> {
        ::protobuf::Optional::new(self.job_removed(), self.has_job_removed())
  }
  pub fn job_removed(&self) -> super::JobRemovedView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(3)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobRemovedView::default())
  }
  pub fn job_removed_mut(&mut self) -> super::JobRemovedMut<'_> {
     let ptr = unsafe {
       self.inner.ptr_mut().get_or_create_mutable_message_at_index(
         3, self.inner.arena()
       ).unwrap()
     };
     ::protobuf::__internal::runtime::MessageMutInner::from_parent(
         self.as_message_mut_inner(::protobuf::__internal::Private),
         ptr
     ).into()
  }
  pub fn set_job_removed(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobRemoved>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        3,
        val
      );
    }
  }

  pub fn event(&self) -> super::job_event::EventOneof<'_> {
    match &self.event_case() {
      super::job_event::EventCase::JobAdded =>
          super::job_event::EventOneof::JobAdded(self.job_added()),
      super::job_event::EventCase::JobPaused =>
          super::job_event::EventOneof::JobPaused(self.job_paused()),
      super::job_event::EventCase::JobResumed =>
          super::job_event::EventOneof::JobResumed(self.job_resumed()),
      super::job_event::EventCase::JobRemoved =>
          super::job_event::EventOneof::JobRemoved(self.job_removed()),
      _ => super::job_event::EventOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn event_case(&self) -> super::job_event::EventCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_event::EventCase::try_from(field_num).unwrap_unchecked()
    }
  }
}  // impl JobEvent

impl ::std::ops::Drop for JobEvent {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobEvent {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobEvent {
  type Proxied = Self;
  fn as_view(&self) -> JobEventView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobEvent {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobEventMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobEvent {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobEvent_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$3333^!|#|$|%");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobEvent_msg_init.0, &[<super::JobAdded as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            <super::JobPaused as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            <super::JobResumed as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            <super::JobRemoved as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            ], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobEvent_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobEvent {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobEvent {
  type Msg = JobEvent;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobEvent> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobEvent {
  type Msg = JobEvent;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobEvent> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobEventMut<'_> {
  type Msg = JobEvent;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobEvent> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobEventMut<'_> {
  type Msg = JobEvent;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobEvent> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobEventView<'_> {
  type Msg = JobEvent;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobEvent> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobEventMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

pub mod job_event {

#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
#[repr(u32)]
pub enum EventOneof<'msg> {
  JobAdded(::protobuf::View<'msg, super::super::JobAdded>) = 1,
  JobPaused(::protobuf::View<'msg, super::super::JobPaused>) = 2,
  JobResumed(::protobuf::View<'msg, super::super::JobResumed>) = 3,
  JobRemoved(::protobuf::View<'msg, super::super::JobRemoved>) = 4,

  not_set(std::marker::PhantomData<&'msg ()>) = 0
}
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(dead_code)]
pub enum EventCase {
  JobAdded = 1,
  JobPaused = 2,
  JobResumed = 3,
  JobRemoved = 4,

  not_set = 0
}

impl EventCase {
  #[allow(dead_code)]
  pub(crate) fn try_from(v: u32) -> ::std::option::Option<EventCase> {
    match v {
      0 => Some(EventCase::not_set),
      1 => Some(EventCase::JobAdded),
      2 => Some(EventCase::JobPaused),
      3 => Some(EventCase::JobResumed),
      4 => Some(EventCase::JobRemoved),
      _ => None
    }
  }
}
}  // pub mod job_event


