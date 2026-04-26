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


// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobAdded_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobAdded {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobAdded>
}

impl ::protobuf::Message for JobAdded {}

impl ::std::default::Default for JobAdded {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobAdded {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobAdded` is `Sync` because it does not implement interior mutability.
//    Neither does `JobAddedMut`.
unsafe impl Sync for JobAdded {}

// SAFETY:
// - `JobAdded` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobAdded {}

impl ::protobuf::Proxied for JobAdded {
  type View<'msg> = JobAddedView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobAdded {}

impl ::protobuf::MutProxied for JobAdded {
  type Mut<'msg> = JobAddedMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobAddedView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobAdded>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobAddedView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobAddedView<'msg> {
  type Message = JobAdded;
}

impl ::std::fmt::Debug for JobAddedView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobAddedView<'_> {
  fn default() -> JobAddedView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobAdded>> for JobAddedView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobAdded>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobAddedView<'msg> {

  pub fn to_owned(&self) -> JobAdded {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // id: optional string
  pub fn has_id(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn id_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

  // job: optional message trogon.cron.jobs.v1.JobDetails
  pub fn has_job(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn job_opt(self) -> ::protobuf::Optional<super::JobDetailsView<'msg>> {
        ::protobuf::Optional::new(self.job(), self.has_job())
  }
  pub fn job(self) -> super::JobDetailsView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobDetailsView::default())
  }

}

// SAFETY:
// - `JobAddedView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobAddedView<'_> {}

// SAFETY:
// - `JobAddedView` is `Send` because while its alive a `JobAddedMut` cannot.
// - `JobAddedView` does not use thread-local data.
unsafe impl Send for JobAddedView<'_> {}

impl<'msg> ::protobuf::AsView for JobAddedView<'msg> {
  type Proxied = JobAdded;
  fn as_view(&self) -> ::protobuf::View<'msg, JobAdded> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobAddedView<'msg> {
  fn into_view<'shorter>(self) -> JobAddedView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobAdded> for JobAddedView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobAdded {
    let mut dst = JobAdded::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobAdded> for JobAddedMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobAdded {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobAdded {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobAddedView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobAddedMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobAddedMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobAdded>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobAddedMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobAddedMut<'msg> {
  type Message = JobAdded;
}

impl ::std::fmt::Debug for JobAddedMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobAdded>> for JobAddedMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobAdded>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobAddedMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobAdded> {
    self.inner
  }

  pub fn to_owned(&self) -> JobAdded {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // id: optional string
  pub fn has_id(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_id(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn id_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_id(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // job: optional message trogon.cron.jobs.v1.JobDetails
  pub fn has_job(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_job(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn job_opt(&self) -> ::protobuf::Optional<super::JobDetailsView<'_>> {
        ::protobuf::Optional::new(self.job(), self.has_job())
  }
  pub fn job(&self) -> super::JobDetailsView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobDetailsView::default())
  }
  pub fn job_mut(&mut self) -> super::JobDetailsMut<'_> {
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
  pub fn set_job(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobDetails>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val
      );
    }
  }

}

// SAFETY:
// - `JobAddedMut` does not perform any shared mutation.
unsafe impl Send for JobAddedMut<'_> {}

// SAFETY:
// - `JobAddedMut` does not perform any shared mutation.
unsafe impl Sync for JobAddedMut<'_> {}

impl<'msg> ::protobuf::AsView for JobAddedMut<'msg> {
  type Proxied = JobAdded;
  fn as_view(&self) -> ::protobuf::View<'_, JobAdded> {
    JobAddedView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobAddedMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobAdded>
  where
      'msg: 'shorter {
    JobAddedView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobAddedMut<'msg> {
  type MutProxied = JobAdded;
  fn as_mut(&mut self) -> JobAddedMut<'msg> {
    JobAddedMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobAddedMut<'msg> {
  fn into_mut<'shorter>(self) -> JobAddedMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobAdded {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobAdded> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobAddedView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobAddedMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // id: optional string
  pub fn has_id(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_id(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn id_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_id(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // job: optional message trogon.cron.jobs.v1.JobDetails
  pub fn has_job(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_job(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn job_opt(&self) -> ::protobuf::Optional<super::JobDetailsView<'_>> {
        ::protobuf::Optional::new(self.job(), self.has_job())
  }
  pub fn job(&self) -> super::JobDetailsView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobDetailsView::default())
  }
  pub fn job_mut(&mut self) -> super::JobDetailsMut<'_> {
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
  pub fn set_job(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobDetails>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val
      );
    }
  }

}  // impl JobAdded

impl ::std::ops::Drop for JobAdded {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobAdded {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobAdded {
  type Proxied = Self;
  fn as_view(&self) -> JobAddedView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobAdded {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobAddedMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobAdded {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobAdded_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$1T3");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobAdded_msg_init.0, &[<super::JobDetails as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            ], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobAdded_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobAdded {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobAdded {
  type Msg = JobAdded;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobAdded> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobAdded {
  type Msg = JobAdded;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobAdded> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobAddedMut<'_> {
  type Msg = JobAdded;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobAdded> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobAddedMut<'_> {
  type Msg = JobAdded;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobAdded> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobAddedView<'_> {
  type Msg = JobAdded;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobAdded> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobAddedMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobPaused_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobPaused {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobPaused>
}

impl ::protobuf::Message for JobPaused {}

impl ::std::default::Default for JobPaused {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobPaused {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobPaused` is `Sync` because it does not implement interior mutability.
//    Neither does `JobPausedMut`.
unsafe impl Sync for JobPaused {}

// SAFETY:
// - `JobPaused` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobPaused {}

impl ::protobuf::Proxied for JobPaused {
  type View<'msg> = JobPausedView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobPaused {}

impl ::protobuf::MutProxied for JobPaused {
  type Mut<'msg> = JobPausedMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobPausedView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobPaused>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobPausedView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobPausedView<'msg> {
  type Message = JobPaused;
}

impl ::std::fmt::Debug for JobPausedView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobPausedView<'_> {
  fn default() -> JobPausedView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobPaused>> for JobPausedView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobPaused>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobPausedView<'msg> {

  pub fn to_owned(&self) -> JobPaused {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // id: optional string
  pub fn has_id(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn id_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

}

// SAFETY:
// - `JobPausedView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobPausedView<'_> {}

// SAFETY:
// - `JobPausedView` is `Send` because while its alive a `JobPausedMut` cannot.
// - `JobPausedView` does not use thread-local data.
unsafe impl Send for JobPausedView<'_> {}

impl<'msg> ::protobuf::AsView for JobPausedView<'msg> {
  type Proxied = JobPaused;
  fn as_view(&self) -> ::protobuf::View<'msg, JobPaused> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobPausedView<'msg> {
  fn into_view<'shorter>(self) -> JobPausedView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobPaused> for JobPausedView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobPaused {
    let mut dst = JobPaused::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobPaused> for JobPausedMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobPaused {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobPaused {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobPausedView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobPausedMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobPausedMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobPaused>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobPausedMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobPausedMut<'msg> {
  type Message = JobPaused;
}

impl ::std::fmt::Debug for JobPausedMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobPaused>> for JobPausedMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobPaused>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobPausedMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobPaused> {
    self.inner
  }

  pub fn to_owned(&self) -> JobPaused {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // id: optional string
  pub fn has_id(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_id(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn id_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_id(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}

// SAFETY:
// - `JobPausedMut` does not perform any shared mutation.
unsafe impl Send for JobPausedMut<'_> {}

// SAFETY:
// - `JobPausedMut` does not perform any shared mutation.
unsafe impl Sync for JobPausedMut<'_> {}

impl<'msg> ::protobuf::AsView for JobPausedMut<'msg> {
  type Proxied = JobPaused;
  fn as_view(&self) -> ::protobuf::View<'_, JobPaused> {
    JobPausedView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobPausedMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobPaused>
  where
      'msg: 'shorter {
    JobPausedView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobPausedMut<'msg> {
  type MutProxied = JobPaused;
  fn as_mut(&mut self) -> JobPausedMut<'msg> {
    JobPausedMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobPausedMut<'msg> {
  fn into_mut<'shorter>(self) -> JobPausedMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobPaused {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobPaused> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobPausedView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobPausedMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // id: optional string
  pub fn has_id(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_id(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn id_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_id(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}  // impl JobPaused

impl ::std::ops::Drop for JobPaused {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobPaused {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobPaused {
  type Proxied = Self;
  fn as_view(&self) -> JobPausedView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobPaused {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobPausedMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobPaused {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobPaused_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$M1");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobPaused_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobPaused_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobPaused {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobPaused {
  type Msg = JobPaused;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobPaused> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobPaused {
  type Msg = JobPaused;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobPaused> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobPausedMut<'_> {
  type Msg = JobPaused;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobPaused> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobPausedMut<'_> {
  type Msg = JobPaused;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobPaused> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobPausedView<'_> {
  type Msg = JobPaused;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobPaused> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobPausedMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobResumed_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobResumed {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobResumed>
}

impl ::protobuf::Message for JobResumed {}

impl ::std::default::Default for JobResumed {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobResumed {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobResumed` is `Sync` because it does not implement interior mutability.
//    Neither does `JobResumedMut`.
unsafe impl Sync for JobResumed {}

// SAFETY:
// - `JobResumed` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobResumed {}

impl ::protobuf::Proxied for JobResumed {
  type View<'msg> = JobResumedView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobResumed {}

impl ::protobuf::MutProxied for JobResumed {
  type Mut<'msg> = JobResumedMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobResumedView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobResumed>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobResumedView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobResumedView<'msg> {
  type Message = JobResumed;
}

impl ::std::fmt::Debug for JobResumedView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobResumedView<'_> {
  fn default() -> JobResumedView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobResumed>> for JobResumedView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobResumed>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobResumedView<'msg> {

  pub fn to_owned(&self) -> JobResumed {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // id: optional string
  pub fn has_id(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn id_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

}

// SAFETY:
// - `JobResumedView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobResumedView<'_> {}

// SAFETY:
// - `JobResumedView` is `Send` because while its alive a `JobResumedMut` cannot.
// - `JobResumedView` does not use thread-local data.
unsafe impl Send for JobResumedView<'_> {}

impl<'msg> ::protobuf::AsView for JobResumedView<'msg> {
  type Proxied = JobResumed;
  fn as_view(&self) -> ::protobuf::View<'msg, JobResumed> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobResumedView<'msg> {
  fn into_view<'shorter>(self) -> JobResumedView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobResumed> for JobResumedView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobResumed {
    let mut dst = JobResumed::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobResumed> for JobResumedMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobResumed {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobResumed {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobResumedView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobResumedMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobResumedMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobResumed>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobResumedMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobResumedMut<'msg> {
  type Message = JobResumed;
}

impl ::std::fmt::Debug for JobResumedMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobResumed>> for JobResumedMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobResumed>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobResumedMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobResumed> {
    self.inner
  }

  pub fn to_owned(&self) -> JobResumed {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // id: optional string
  pub fn has_id(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_id(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn id_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_id(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}

// SAFETY:
// - `JobResumedMut` does not perform any shared mutation.
unsafe impl Send for JobResumedMut<'_> {}

// SAFETY:
// - `JobResumedMut` does not perform any shared mutation.
unsafe impl Sync for JobResumedMut<'_> {}

impl<'msg> ::protobuf::AsView for JobResumedMut<'msg> {
  type Proxied = JobResumed;
  fn as_view(&self) -> ::protobuf::View<'_, JobResumed> {
    JobResumedView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobResumedMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobResumed>
  where
      'msg: 'shorter {
    JobResumedView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobResumedMut<'msg> {
  type MutProxied = JobResumed;
  fn as_mut(&mut self) -> JobResumedMut<'msg> {
    JobResumedMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobResumedMut<'msg> {
  fn into_mut<'shorter>(self) -> JobResumedMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobResumed {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobResumed> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobResumedView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobResumedMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // id: optional string
  pub fn has_id(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_id(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn id_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_id(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}  // impl JobResumed

impl ::std::ops::Drop for JobResumed {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobResumed {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobResumed {
  type Proxied = Self;
  fn as_view(&self) -> JobResumedView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobResumed {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobResumedMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobResumed {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobResumed_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$M1");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobResumed_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobResumed_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobResumed {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobResumed {
  type Msg = JobResumed;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobResumed> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobResumed {
  type Msg = JobResumed;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobResumed> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobResumedMut<'_> {
  type Msg = JobResumed;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobResumed> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobResumedMut<'_> {
  type Msg = JobResumed;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobResumed> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobResumedView<'_> {
  type Msg = JobResumed;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobResumed> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobResumedMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobRemoved_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobRemoved {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobRemoved>
}

impl ::protobuf::Message for JobRemoved {}

impl ::std::default::Default for JobRemoved {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobRemoved {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobRemoved` is `Sync` because it does not implement interior mutability.
//    Neither does `JobRemovedMut`.
unsafe impl Sync for JobRemoved {}

// SAFETY:
// - `JobRemoved` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobRemoved {}

impl ::protobuf::Proxied for JobRemoved {
  type View<'msg> = JobRemovedView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobRemoved {}

impl ::protobuf::MutProxied for JobRemoved {
  type Mut<'msg> = JobRemovedMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobRemovedView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobRemoved>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobRemovedView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobRemovedView<'msg> {
  type Message = JobRemoved;
}

impl ::std::fmt::Debug for JobRemovedView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobRemovedView<'_> {
  fn default() -> JobRemovedView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobRemoved>> for JobRemovedView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobRemoved>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobRemovedView<'msg> {

  pub fn to_owned(&self) -> JobRemoved {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // id: optional string
  pub fn has_id(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn id_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

}

// SAFETY:
// - `JobRemovedView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobRemovedView<'_> {}

// SAFETY:
// - `JobRemovedView` is `Send` because while its alive a `JobRemovedMut` cannot.
// - `JobRemovedView` does not use thread-local data.
unsafe impl Send for JobRemovedView<'_> {}

impl<'msg> ::protobuf::AsView for JobRemovedView<'msg> {
  type Proxied = JobRemoved;
  fn as_view(&self) -> ::protobuf::View<'msg, JobRemoved> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobRemovedView<'msg> {
  fn into_view<'shorter>(self) -> JobRemovedView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobRemoved> for JobRemovedView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobRemoved {
    let mut dst = JobRemoved::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobRemoved> for JobRemovedMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobRemoved {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobRemoved {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobRemovedView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobRemovedMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobRemovedMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobRemoved>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobRemovedMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobRemovedMut<'msg> {
  type Message = JobRemoved;
}

impl ::std::fmt::Debug for JobRemovedMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobRemoved>> for JobRemovedMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobRemoved>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobRemovedMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobRemoved> {
    self.inner
  }

  pub fn to_owned(&self) -> JobRemoved {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // id: optional string
  pub fn has_id(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_id(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn id_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_id(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}

// SAFETY:
// - `JobRemovedMut` does not perform any shared mutation.
unsafe impl Send for JobRemovedMut<'_> {}

// SAFETY:
// - `JobRemovedMut` does not perform any shared mutation.
unsafe impl Sync for JobRemovedMut<'_> {}

impl<'msg> ::protobuf::AsView for JobRemovedMut<'msg> {
  type Proxied = JobRemoved;
  fn as_view(&self) -> ::protobuf::View<'_, JobRemoved> {
    JobRemovedView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobRemovedMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobRemoved>
  where
      'msg: 'shorter {
    JobRemovedView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobRemovedMut<'msg> {
  type MutProxied = JobRemoved;
  fn as_mut(&mut self) -> JobRemovedMut<'msg> {
    JobRemovedMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobRemovedMut<'msg> {
  fn into_mut<'shorter>(self) -> JobRemovedMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobRemoved {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobRemoved> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobRemovedView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobRemovedMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // id: optional string
  pub fn has_id(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_id(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn id_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.id(), self.has_id())
  }
  pub fn id(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_id(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}  // impl JobRemoved

impl ::std::ops::Drop for JobRemoved {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobRemoved {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobRemoved {
  type Proxied = Self;
  fn as_view(&self) -> JobRemovedView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobRemoved {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobRemovedMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobRemoved {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobRemoved_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$M1");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobRemoved_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobRemoved_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobRemoved {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobRemoved {
  type Msg = JobRemoved;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobRemoved> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobRemoved {
  type Msg = JobRemoved;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobRemoved> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobRemovedMut<'_> {
  type Msg = JobRemoved;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobRemoved> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobRemovedMut<'_> {
  type Msg = JobRemoved;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobRemoved> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobRemovedView<'_> {
  type Msg = JobRemoved;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobRemoved> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobRemovedMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobDetails_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobDetails {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobDetails>
}

impl ::protobuf::Message for JobDetails {}

impl ::std::default::Default for JobDetails {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobDetails {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobDetails` is `Sync` because it does not implement interior mutability.
//    Neither does `JobDetailsMut`.
unsafe impl Sync for JobDetails {}

// SAFETY:
// - `JobDetails` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobDetails {}

impl ::protobuf::Proxied for JobDetails {
  type View<'msg> = JobDetailsView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobDetails {}

impl ::protobuf::MutProxied for JobDetails {
  type Mut<'msg> = JobDetailsMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobDetailsView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobDetails>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobDetailsView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobDetailsView<'msg> {
  type Message = JobDetails;
}

impl ::std::fmt::Debug for JobDetailsView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobDetailsView<'_> {
  fn default() -> JobDetailsView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobDetails>> for JobDetailsView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobDetails>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobDetailsView<'msg> {

  pub fn to_owned(&self) -> JobDetails {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // status: optional enum trogon.cron.jobs.v1.JobStatus
  pub fn has_status(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn status_opt(self) -> ::protobuf::Optional<super::JobStatus> {
        ::protobuf::Optional::new(self.status(), self.has_status())
  }
  pub fn status(self) -> super::JobStatus {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::JobStatus::Unspecified).into()
      ).try_into().unwrap()
    }
  }

  // schedule: optional message trogon.cron.jobs.v1.JobSchedule
  pub fn has_schedule(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn schedule_opt(self) -> ::protobuf::Optional<super::JobScheduleView<'msg>> {
        ::protobuf::Optional::new(self.schedule(), self.has_schedule())
  }
  pub fn schedule(self) -> super::JobScheduleView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobScheduleView::default())
  }

  // delivery: optional message trogon.cron.jobs.v1.JobDelivery
  pub fn has_delivery(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn delivery_opt(self) -> ::protobuf::Optional<super::JobDeliveryView<'msg>> {
        ::protobuf::Optional::new(self.delivery(), self.has_delivery())
  }
  pub fn delivery(self) -> super::JobDeliveryView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobDeliveryView::default())
  }

  // message: optional message trogon.cron.jobs.v1.JobMessage
  pub fn has_message(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(3)
    }
  }
  pub fn message_opt(self) -> ::protobuf::Optional<super::JobMessageView<'msg>> {
        ::protobuf::Optional::new(self.message(), self.has_message())
  }
  pub fn message(self) -> super::JobMessageView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(3)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobMessageView::default())
  }

}

// SAFETY:
// - `JobDetailsView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobDetailsView<'_> {}

// SAFETY:
// - `JobDetailsView` is `Send` because while its alive a `JobDetailsMut` cannot.
// - `JobDetailsView` does not use thread-local data.
unsafe impl Send for JobDetailsView<'_> {}

impl<'msg> ::protobuf::AsView for JobDetailsView<'msg> {
  type Proxied = JobDetails;
  fn as_view(&self) -> ::protobuf::View<'msg, JobDetails> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobDetailsView<'msg> {
  fn into_view<'shorter>(self) -> JobDetailsView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobDetails> for JobDetailsView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobDetails {
    let mut dst = JobDetails::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobDetails> for JobDetailsMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobDetails {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobDetails {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobDetailsView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobDetailsMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobDetailsMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobDetails>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobDetailsMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobDetailsMut<'msg> {
  type Message = JobDetails;
}

impl ::std::fmt::Debug for JobDetailsMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobDetails>> for JobDetailsMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobDetails>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobDetailsMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobDetails> {
    self.inner
  }

  pub fn to_owned(&self) -> JobDetails {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // status: optional enum trogon.cron.jobs.v1.JobStatus
  pub fn has_status(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_status(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn status_opt(&self) -> ::protobuf::Optional<super::JobStatus> {
        ::protobuf::Optional::new(self.status(), self.has_status())
  }
  pub fn status(&self) -> super::JobStatus {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::JobStatus::Unspecified).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_status(&mut self, val: super::JobStatus) {
    unsafe {
      // TODO: b/361751487: This .into() is only here
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      //perfectly.
      self.inner.ptr_mut().set_base_field_i32_at_index(
        0, val.into()
      )
    }
  }

  // schedule: optional message trogon.cron.jobs.v1.JobSchedule
  pub fn has_schedule(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_schedule(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn schedule_opt(&self) -> ::protobuf::Optional<super::JobScheduleView<'_>> {
        ::protobuf::Optional::new(self.schedule(), self.has_schedule())
  }
  pub fn schedule(&self) -> super::JobScheduleView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobScheduleView::default())
  }
  pub fn schedule_mut(&mut self) -> super::JobScheduleMut<'_> {
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
  pub fn set_schedule(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobSchedule>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val
      );
    }
  }

  // delivery: optional message trogon.cron.jobs.v1.JobDelivery
  pub fn has_delivery(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn clear_delivery(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        2
      );
    }
  }
  pub fn delivery_opt(&self) -> ::protobuf::Optional<super::JobDeliveryView<'_>> {
        ::protobuf::Optional::new(self.delivery(), self.has_delivery())
  }
  pub fn delivery(&self) -> super::JobDeliveryView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobDeliveryView::default())
  }
  pub fn delivery_mut(&mut self) -> super::JobDeliveryMut<'_> {
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
  pub fn set_delivery(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobDelivery>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        2,
        val
      );
    }
  }

  // message: optional message trogon.cron.jobs.v1.JobMessage
  pub fn has_message(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(3)
    }
  }
  pub fn clear_message(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        3
      );
    }
  }
  pub fn message_opt(&self) -> ::protobuf::Optional<super::JobMessageView<'_>> {
        ::protobuf::Optional::new(self.message(), self.has_message())
  }
  pub fn message(&self) -> super::JobMessageView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(3)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobMessageView::default())
  }
  pub fn message_mut(&mut self) -> super::JobMessageMut<'_> {
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
  pub fn set_message(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobMessage>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        3,
        val
      );
    }
  }

}

// SAFETY:
// - `JobDetailsMut` does not perform any shared mutation.
unsafe impl Send for JobDetailsMut<'_> {}

// SAFETY:
// - `JobDetailsMut` does not perform any shared mutation.
unsafe impl Sync for JobDetailsMut<'_> {}

impl<'msg> ::protobuf::AsView for JobDetailsMut<'msg> {
  type Proxied = JobDetails;
  fn as_view(&self) -> ::protobuf::View<'_, JobDetails> {
    JobDetailsView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobDetailsMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobDetails>
  where
      'msg: 'shorter {
    JobDetailsView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobDetailsMut<'msg> {
  type MutProxied = JobDetails;
  fn as_mut(&mut self) -> JobDetailsMut<'msg> {
    JobDetailsMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobDetailsMut<'msg> {
  fn into_mut<'shorter>(self) -> JobDetailsMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobDetails {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobDetails> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobDetailsView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobDetailsMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // status: optional enum trogon.cron.jobs.v1.JobStatus
  pub fn has_status(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_status(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn status_opt(&self) -> ::protobuf::Optional<super::JobStatus> {
        ::protobuf::Optional::new(self.status(), self.has_status())
  }
  pub fn status(&self) -> super::JobStatus {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::JobStatus::Unspecified).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_status(&mut self, val: super::JobStatus) {
    unsafe {
      // TODO: b/361751487: This .into() is only here
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      //perfectly.
      self.inner.ptr_mut().set_base_field_i32_at_index(
        0, val.into()
      )
    }
  }

  // schedule: optional message trogon.cron.jobs.v1.JobSchedule
  pub fn has_schedule(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_schedule(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn schedule_opt(&self) -> ::protobuf::Optional<super::JobScheduleView<'_>> {
        ::protobuf::Optional::new(self.schedule(), self.has_schedule())
  }
  pub fn schedule(&self) -> super::JobScheduleView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobScheduleView::default())
  }
  pub fn schedule_mut(&mut self) -> super::JobScheduleMut<'_> {
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
  pub fn set_schedule(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobSchedule>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val
      );
    }
  }

  // delivery: optional message trogon.cron.jobs.v1.JobDelivery
  pub fn has_delivery(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn clear_delivery(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        2
      );
    }
  }
  pub fn delivery_opt(&self) -> ::protobuf::Optional<super::JobDeliveryView<'_>> {
        ::protobuf::Optional::new(self.delivery(), self.has_delivery())
  }
  pub fn delivery(&self) -> super::JobDeliveryView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobDeliveryView::default())
  }
  pub fn delivery_mut(&mut self) -> super::JobDeliveryMut<'_> {
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
  pub fn set_delivery(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobDelivery>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        2,
        val
      );
    }
  }

  // message: optional message trogon.cron.jobs.v1.JobMessage
  pub fn has_message(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(3)
    }
  }
  pub fn clear_message(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        3
      );
    }
  }
  pub fn message_opt(&self) -> ::protobuf::Optional<super::JobMessageView<'_>> {
        ::protobuf::Optional::new(self.message(), self.has_message())
  }
  pub fn message(&self) -> super::JobMessageView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(3)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobMessageView::default())
  }
  pub fn message_mut(&mut self) -> super::JobMessageMut<'_> {
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
  pub fn set_message(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobMessage>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        3,
        val
      );
    }
  }

}  // impl JobDetails

impl ::std::ops::Drop for JobDetails {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobDetails {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobDetails {
  type Proxied = Self;
  fn as_view(&self) -> JobDetailsView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobDetails {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobDetailsMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobDetails {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobDetails_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$.333");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobDetails_msg_init.0, &[<super::JobSchedule as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            <super::JobDelivery as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            <super::JobMessage as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            ], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobDetails_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobDetails {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobDetails {
  type Msg = JobDetails;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDetails> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobDetails {
  type Msg = JobDetails;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDetails> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobDetailsMut<'_> {
  type Msg = JobDetails;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDetails> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobDetailsMut<'_> {
  type Msg = JobDetails;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDetails> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobDetailsView<'_> {
  type Msg = JobDetails;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDetails> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobDetailsMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobSchedule_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobSchedule {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobSchedule>
}

impl ::protobuf::Message for JobSchedule {}

impl ::std::default::Default for JobSchedule {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobSchedule {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobSchedule` is `Sync` because it does not implement interior mutability.
//    Neither does `JobScheduleMut`.
unsafe impl Sync for JobSchedule {}

// SAFETY:
// - `JobSchedule` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobSchedule {}

impl ::protobuf::Proxied for JobSchedule {
  type View<'msg> = JobScheduleView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobSchedule {}

impl ::protobuf::MutProxied for JobSchedule {
  type Mut<'msg> = JobScheduleMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobScheduleView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobSchedule>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobScheduleView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobScheduleView<'msg> {
  type Message = JobSchedule;
}

impl ::std::fmt::Debug for JobScheduleView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobScheduleView<'_> {
  fn default() -> JobScheduleView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobSchedule>> for JobScheduleView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobSchedule>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobScheduleView<'msg> {

  pub fn to_owned(&self) -> JobSchedule {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // at: optional message trogon.cron.jobs.v1.AtSchedule
  pub fn has_at(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn at_opt(self) -> ::protobuf::Optional<super::AtScheduleView<'msg>> {
        ::protobuf::Optional::new(self.at(), self.has_at())
  }
  pub fn at(self) -> super::AtScheduleView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::AtScheduleView::default())
  }

  // every: optional message trogon.cron.jobs.v1.EverySchedule
  pub fn has_every(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn every_opt(self) -> ::protobuf::Optional<super::EveryScheduleView<'msg>> {
        ::protobuf::Optional::new(self.every(), self.has_every())
  }
  pub fn every(self) -> super::EveryScheduleView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::EveryScheduleView::default())
  }

  // cron: optional message trogon.cron.jobs.v1.CronSchedule
  pub fn has_cron(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn cron_opt(self) -> ::protobuf::Optional<super::CronScheduleView<'msg>> {
        ::protobuf::Optional::new(self.cron(), self.has_cron())
  }
  pub fn cron(self) -> super::CronScheduleView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::CronScheduleView::default())
  }

  pub fn kind(self) -> super::job_schedule::KindOneof<'msg> {
    match self.kind_case() {
      super::job_schedule::KindCase::At =>
          super::job_schedule::KindOneof::At(self.at()),
      super::job_schedule::KindCase::Every =>
          super::job_schedule::KindOneof::Every(self.every()),
      super::job_schedule::KindCase::Cron =>
          super::job_schedule::KindOneof::Cron(self.cron()),
      _ => super::job_schedule::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(self) -> super::job_schedule::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_schedule::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}

// SAFETY:
// - `JobScheduleView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobScheduleView<'_> {}

// SAFETY:
// - `JobScheduleView` is `Send` because while its alive a `JobScheduleMut` cannot.
// - `JobScheduleView` does not use thread-local data.
unsafe impl Send for JobScheduleView<'_> {}

impl<'msg> ::protobuf::AsView for JobScheduleView<'msg> {
  type Proxied = JobSchedule;
  fn as_view(&self) -> ::protobuf::View<'msg, JobSchedule> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobScheduleView<'msg> {
  fn into_view<'shorter>(self) -> JobScheduleView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobSchedule> for JobScheduleView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobSchedule {
    let mut dst = JobSchedule::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobSchedule> for JobScheduleMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobSchedule {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobSchedule {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobScheduleView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobScheduleMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobScheduleMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobSchedule>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobScheduleMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobScheduleMut<'msg> {
  type Message = JobSchedule;
}

impl ::std::fmt::Debug for JobScheduleMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobSchedule>> for JobScheduleMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobSchedule>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobScheduleMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobSchedule> {
    self.inner
  }

  pub fn to_owned(&self) -> JobSchedule {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // at: optional message trogon.cron.jobs.v1.AtSchedule
  pub fn has_at(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_at(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn at_opt(&self) -> ::protobuf::Optional<super::AtScheduleView<'_>> {
        ::protobuf::Optional::new(self.at(), self.has_at())
  }
  pub fn at(&self) -> super::AtScheduleView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::AtScheduleView::default())
  }
  pub fn at_mut(&mut self) -> super::AtScheduleMut<'_> {
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
  pub fn set_at(&mut self,
    val: impl ::protobuf::IntoProxied<super::AtSchedule>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val
      );
    }
  }

  // every: optional message trogon.cron.jobs.v1.EverySchedule
  pub fn has_every(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_every(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn every_opt(&self) -> ::protobuf::Optional<super::EveryScheduleView<'_>> {
        ::protobuf::Optional::new(self.every(), self.has_every())
  }
  pub fn every(&self) -> super::EveryScheduleView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::EveryScheduleView::default())
  }
  pub fn every_mut(&mut self) -> super::EveryScheduleMut<'_> {
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
  pub fn set_every(&mut self,
    val: impl ::protobuf::IntoProxied<super::EverySchedule>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val
      );
    }
  }

  // cron: optional message trogon.cron.jobs.v1.CronSchedule
  pub fn has_cron(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn clear_cron(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        2
      );
    }
  }
  pub fn cron_opt(&self) -> ::protobuf::Optional<super::CronScheduleView<'_>> {
        ::protobuf::Optional::new(self.cron(), self.has_cron())
  }
  pub fn cron(&self) -> super::CronScheduleView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::CronScheduleView::default())
  }
  pub fn cron_mut(&mut self) -> super::CronScheduleMut<'_> {
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
  pub fn set_cron(&mut self,
    val: impl ::protobuf::IntoProxied<super::CronSchedule>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        2,
        val
      );
    }
  }

  pub fn kind(&self) -> super::job_schedule::KindOneof<'_> {
    match &self.kind_case() {
      super::job_schedule::KindCase::At =>
          super::job_schedule::KindOneof::At(self.at()),
      super::job_schedule::KindCase::Every =>
          super::job_schedule::KindOneof::Every(self.every()),
      super::job_schedule::KindCase::Cron =>
          super::job_schedule::KindOneof::Cron(self.cron()),
      _ => super::job_schedule::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(&self) -> super::job_schedule::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_schedule::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}

// SAFETY:
// - `JobScheduleMut` does not perform any shared mutation.
unsafe impl Send for JobScheduleMut<'_> {}

// SAFETY:
// - `JobScheduleMut` does not perform any shared mutation.
unsafe impl Sync for JobScheduleMut<'_> {}

impl<'msg> ::protobuf::AsView for JobScheduleMut<'msg> {
  type Proxied = JobSchedule;
  fn as_view(&self) -> ::protobuf::View<'_, JobSchedule> {
    JobScheduleView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobScheduleMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobSchedule>
  where
      'msg: 'shorter {
    JobScheduleView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobScheduleMut<'msg> {
  type MutProxied = JobSchedule;
  fn as_mut(&mut self) -> JobScheduleMut<'msg> {
    JobScheduleMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobScheduleMut<'msg> {
  fn into_mut<'shorter>(self) -> JobScheduleMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobSchedule {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobSchedule> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobScheduleView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobScheduleMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // at: optional message trogon.cron.jobs.v1.AtSchedule
  pub fn has_at(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_at(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn at_opt(&self) -> ::protobuf::Optional<super::AtScheduleView<'_>> {
        ::protobuf::Optional::new(self.at(), self.has_at())
  }
  pub fn at(&self) -> super::AtScheduleView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::AtScheduleView::default())
  }
  pub fn at_mut(&mut self) -> super::AtScheduleMut<'_> {
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
  pub fn set_at(&mut self,
    val: impl ::protobuf::IntoProxied<super::AtSchedule>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val
      );
    }
  }

  // every: optional message trogon.cron.jobs.v1.EverySchedule
  pub fn has_every(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_every(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn every_opt(&self) -> ::protobuf::Optional<super::EveryScheduleView<'_>> {
        ::protobuf::Optional::new(self.every(), self.has_every())
  }
  pub fn every(&self) -> super::EveryScheduleView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(1)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::EveryScheduleView::default())
  }
  pub fn every_mut(&mut self) -> super::EveryScheduleMut<'_> {
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
  pub fn set_every(&mut self,
    val: impl ::protobuf::IntoProxied<super::EverySchedule>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val
      );
    }
  }

  // cron: optional message trogon.cron.jobs.v1.CronSchedule
  pub fn has_cron(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn clear_cron(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        2
      );
    }
  }
  pub fn cron_opt(&self) -> ::protobuf::Optional<super::CronScheduleView<'_>> {
        ::protobuf::Optional::new(self.cron(), self.has_cron())
  }
  pub fn cron(&self) -> super::CronScheduleView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::CronScheduleView::default())
  }
  pub fn cron_mut(&mut self) -> super::CronScheduleMut<'_> {
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
  pub fn set_cron(&mut self,
    val: impl ::protobuf::IntoProxied<super::CronSchedule>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        2,
        val
      );
    }
  }

  pub fn kind(&self) -> super::job_schedule::KindOneof<'_> {
    match &self.kind_case() {
      super::job_schedule::KindCase::At =>
          super::job_schedule::KindOneof::At(self.at()),
      super::job_schedule::KindCase::Every =>
          super::job_schedule::KindOneof::Every(self.every()),
      super::job_schedule::KindCase::Cron =>
          super::job_schedule::KindOneof::Cron(self.cron()),
      _ => super::job_schedule::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(&self) -> super::job_schedule::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_schedule::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}  // impl JobSchedule

impl ::std::ops::Drop for JobSchedule {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobSchedule {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobSchedule {
  type Proxied = Self;
  fn as_view(&self) -> JobScheduleView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobSchedule {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobScheduleMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobSchedule {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobSchedule_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$333^!|#|$");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobSchedule_msg_init.0, &[<super::AtSchedule as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            <super::EverySchedule as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            <super::CronSchedule as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            ], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobSchedule_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobSchedule {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobSchedule {
  type Msg = JobSchedule;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSchedule> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobSchedule {
  type Msg = JobSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSchedule> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobScheduleMut<'_> {
  type Msg = JobSchedule;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSchedule> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobScheduleMut<'_> {
  type Msg = JobSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSchedule> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobScheduleView<'_> {
  type Msg = JobSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSchedule> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobScheduleMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

pub mod job_schedule {

#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
#[repr(u32)]
pub enum KindOneof<'msg> {
  At(::protobuf::View<'msg, super::super::AtSchedule>) = 1,
  Every(::protobuf::View<'msg, super::super::EverySchedule>) = 2,
  Cron(::protobuf::View<'msg, super::super::CronSchedule>) = 3,

  not_set(std::marker::PhantomData<&'msg ()>) = 0
}
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(dead_code)]
pub enum KindCase {
  At = 1,
  Every = 2,
  Cron = 3,

  not_set = 0
}

impl KindCase {
  #[allow(dead_code)]
  pub(crate) fn try_from(v: u32) -> ::std::option::Option<KindCase> {
    match v {
      0 => Some(KindCase::not_set),
      1 => Some(KindCase::At),
      2 => Some(KindCase::Every),
      3 => Some(KindCase::Cron),
      _ => None
    }
  }
}
}  // pub mod job_schedule


// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__AtSchedule_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct AtSchedule {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<AtSchedule>
}

impl ::protobuf::Message for AtSchedule {}

impl ::std::default::Default for AtSchedule {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for AtSchedule {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `AtSchedule` is `Sync` because it does not implement interior mutability.
//    Neither does `AtScheduleMut`.
unsafe impl Sync for AtSchedule {}

// SAFETY:
// - `AtSchedule` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for AtSchedule {}

impl ::protobuf::Proxied for AtSchedule {
  type View<'msg> = AtScheduleView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for AtSchedule {}

impl ::protobuf::MutProxied for AtSchedule {
  type Mut<'msg> = AtScheduleMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct AtScheduleView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, AtSchedule>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for AtScheduleView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for AtScheduleView<'msg> {
  type Message = AtSchedule;
}

impl ::std::fmt::Debug for AtScheduleView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for AtScheduleView<'_> {
  fn default() -> AtScheduleView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, AtSchedule>> for AtScheduleView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, AtSchedule>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> AtScheduleView<'msg> {

  pub fn to_owned(&self) -> AtSchedule {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // at: optional string
  pub fn has_at(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn at_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.at(), self.has_at())
  }
  pub fn at(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

}

// SAFETY:
// - `AtScheduleView` is `Sync` because it does not support mutation.
unsafe impl Sync for AtScheduleView<'_> {}

// SAFETY:
// - `AtScheduleView` is `Send` because while its alive a `AtScheduleMut` cannot.
// - `AtScheduleView` does not use thread-local data.
unsafe impl Send for AtScheduleView<'_> {}

impl<'msg> ::protobuf::AsView for AtScheduleView<'msg> {
  type Proxied = AtSchedule;
  fn as_view(&self) -> ::protobuf::View<'msg, AtSchedule> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for AtScheduleView<'msg> {
  fn into_view<'shorter>(self) -> AtScheduleView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<AtSchedule> for AtScheduleView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> AtSchedule {
    let mut dst = AtSchedule::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<AtSchedule> for AtScheduleMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> AtSchedule {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for AtSchedule {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for AtScheduleView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for AtScheduleMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct AtScheduleMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, AtSchedule>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for AtScheduleMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for AtScheduleMut<'msg> {
  type Message = AtSchedule;
}

impl ::std::fmt::Debug for AtScheduleMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, AtSchedule>> for AtScheduleMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, AtSchedule>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> AtScheduleMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, AtSchedule> {
    self.inner
  }

  pub fn to_owned(&self) -> AtSchedule {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // at: optional string
  pub fn has_at(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_at(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn at_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.at(), self.has_at())
  }
  pub fn at(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_at(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}

// SAFETY:
// - `AtScheduleMut` does not perform any shared mutation.
unsafe impl Send for AtScheduleMut<'_> {}

// SAFETY:
// - `AtScheduleMut` does not perform any shared mutation.
unsafe impl Sync for AtScheduleMut<'_> {}

impl<'msg> ::protobuf::AsView for AtScheduleMut<'msg> {
  type Proxied = AtSchedule;
  fn as_view(&self) -> ::protobuf::View<'_, AtSchedule> {
    AtScheduleView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for AtScheduleMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, AtSchedule>
  where
      'msg: 'shorter {
    AtScheduleView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for AtScheduleMut<'msg> {
  type MutProxied = AtSchedule;
  fn as_mut(&mut self) -> AtScheduleMut<'msg> {
    AtScheduleMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for AtScheduleMut<'msg> {
  fn into_mut<'shorter>(self) -> AtScheduleMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl AtSchedule {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, AtSchedule> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> AtScheduleView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> AtScheduleMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // at: optional string
  pub fn has_at(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_at(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn at_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.at(), self.has_at())
  }
  pub fn at(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_at(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}  // impl AtSchedule

impl ::std::ops::Drop for AtSchedule {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for AtSchedule {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for AtSchedule {
  type Proxied = Self;
  fn as_view(&self) -> AtScheduleView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for AtSchedule {
  type MutProxied = Self;
  fn as_mut(&mut self) -> AtScheduleMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for AtSchedule {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__AtSchedule_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$M1");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__AtSchedule_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__AtSchedule_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for AtSchedule {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for AtSchedule {
  type Msg = AtSchedule;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<AtSchedule> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for AtSchedule {
  type Msg = AtSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<AtSchedule> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for AtScheduleMut<'_> {
  type Msg = AtSchedule;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<AtSchedule> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for AtScheduleMut<'_> {
  type Msg = AtSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<AtSchedule> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for AtScheduleView<'_> {
  type Msg = AtSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<AtSchedule> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for AtScheduleMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__EverySchedule_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct EverySchedule {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<EverySchedule>
}

impl ::protobuf::Message for EverySchedule {}

impl ::std::default::Default for EverySchedule {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for EverySchedule {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `EverySchedule` is `Sync` because it does not implement interior mutability.
//    Neither does `EveryScheduleMut`.
unsafe impl Sync for EverySchedule {}

// SAFETY:
// - `EverySchedule` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for EverySchedule {}

impl ::protobuf::Proxied for EverySchedule {
  type View<'msg> = EveryScheduleView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for EverySchedule {}

impl ::protobuf::MutProxied for EverySchedule {
  type Mut<'msg> = EveryScheduleMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct EveryScheduleView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, EverySchedule>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for EveryScheduleView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for EveryScheduleView<'msg> {
  type Message = EverySchedule;
}

impl ::std::fmt::Debug for EveryScheduleView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for EveryScheduleView<'_> {
  fn default() -> EveryScheduleView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, EverySchedule>> for EveryScheduleView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, EverySchedule>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> EveryScheduleView<'msg> {

  pub fn to_owned(&self) -> EverySchedule {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // every_sec: optional uint64
  pub fn has_every_sec(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn every_sec_opt(self) -> ::protobuf::Optional<u64> {
        ::protobuf::Optional::new(self.every_sec(), self.has_every_sec())
  }
  pub fn every_sec(self) -> u64 {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_u64_at_index(
        0, (0u64).into()
      ).try_into().unwrap()
    }
  }

}

// SAFETY:
// - `EveryScheduleView` is `Sync` because it does not support mutation.
unsafe impl Sync for EveryScheduleView<'_> {}

// SAFETY:
// - `EveryScheduleView` is `Send` because while its alive a `EveryScheduleMut` cannot.
// - `EveryScheduleView` does not use thread-local data.
unsafe impl Send for EveryScheduleView<'_> {}

impl<'msg> ::protobuf::AsView for EveryScheduleView<'msg> {
  type Proxied = EverySchedule;
  fn as_view(&self) -> ::protobuf::View<'msg, EverySchedule> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for EveryScheduleView<'msg> {
  fn into_view<'shorter>(self) -> EveryScheduleView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<EverySchedule> for EveryScheduleView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> EverySchedule {
    let mut dst = EverySchedule::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<EverySchedule> for EveryScheduleMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> EverySchedule {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for EverySchedule {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for EveryScheduleView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for EveryScheduleMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct EveryScheduleMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, EverySchedule>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for EveryScheduleMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for EveryScheduleMut<'msg> {
  type Message = EverySchedule;
}

impl ::std::fmt::Debug for EveryScheduleMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, EverySchedule>> for EveryScheduleMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, EverySchedule>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> EveryScheduleMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, EverySchedule> {
    self.inner
  }

  pub fn to_owned(&self) -> EverySchedule {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // every_sec: optional uint64
  pub fn has_every_sec(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_every_sec(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn every_sec_opt(&self) -> ::protobuf::Optional<u64> {
        ::protobuf::Optional::new(self.every_sec(), self.has_every_sec())
  }
  pub fn every_sec(&self) -> u64 {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_u64_at_index(
        0, (0u64).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_every_sec(&mut self, val: u64) {
    unsafe {
      // TODO: b/361751487: This .into() is only here
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      //perfectly.
      self.inner.ptr_mut().set_base_field_u64_at_index(
        0, val.into()
      )
    }
  }

}

// SAFETY:
// - `EveryScheduleMut` does not perform any shared mutation.
unsafe impl Send for EveryScheduleMut<'_> {}

// SAFETY:
// - `EveryScheduleMut` does not perform any shared mutation.
unsafe impl Sync for EveryScheduleMut<'_> {}

impl<'msg> ::protobuf::AsView for EveryScheduleMut<'msg> {
  type Proxied = EverySchedule;
  fn as_view(&self) -> ::protobuf::View<'_, EverySchedule> {
    EveryScheduleView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for EveryScheduleMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, EverySchedule>
  where
      'msg: 'shorter {
    EveryScheduleView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for EveryScheduleMut<'msg> {
  type MutProxied = EverySchedule;
  fn as_mut(&mut self) -> EveryScheduleMut<'msg> {
    EveryScheduleMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for EveryScheduleMut<'msg> {
  fn into_mut<'shorter>(self) -> EveryScheduleMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl EverySchedule {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, EverySchedule> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> EveryScheduleView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> EveryScheduleMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // every_sec: optional uint64
  pub fn has_every_sec(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_every_sec(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn every_sec_opt(&self) -> ::protobuf::Optional<u64> {
        ::protobuf::Optional::new(self.every_sec(), self.has_every_sec())
  }
  pub fn every_sec(&self) -> u64 {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_u64_at_index(
        0, (0u64).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_every_sec(&mut self, val: u64) {
    unsafe {
      // TODO: b/361751487: This .into() is only here
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      //perfectly.
      self.inner.ptr_mut().set_base_field_u64_at_index(
        0, val.into()
      )
    }
  }

}  // impl EverySchedule

impl ::std::ops::Drop for EverySchedule {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for EverySchedule {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for EverySchedule {
  type Proxied = Self;
  fn as_view(&self) -> EveryScheduleView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for EverySchedule {
  type MutProxied = Self;
  fn as_mut(&mut self) -> EveryScheduleMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for EverySchedule {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__EverySchedule_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$,");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__EverySchedule_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__EverySchedule_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for EverySchedule {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for EverySchedule {
  type Msg = EverySchedule;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<EverySchedule> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for EverySchedule {
  type Msg = EverySchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<EverySchedule> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for EveryScheduleMut<'_> {
  type Msg = EverySchedule;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<EverySchedule> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for EveryScheduleMut<'_> {
  type Msg = EverySchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<EverySchedule> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for EveryScheduleView<'_> {
  type Msg = EverySchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<EverySchedule> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for EveryScheduleMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__CronSchedule_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct CronSchedule {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<CronSchedule>
}

impl ::protobuf::Message for CronSchedule {}

impl ::std::default::Default for CronSchedule {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for CronSchedule {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `CronSchedule` is `Sync` because it does not implement interior mutability.
//    Neither does `CronScheduleMut`.
unsafe impl Sync for CronSchedule {}

// SAFETY:
// - `CronSchedule` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for CronSchedule {}

impl ::protobuf::Proxied for CronSchedule {
  type View<'msg> = CronScheduleView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for CronSchedule {}

impl ::protobuf::MutProxied for CronSchedule {
  type Mut<'msg> = CronScheduleMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct CronScheduleView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, CronSchedule>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for CronScheduleView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for CronScheduleView<'msg> {
  type Message = CronSchedule;
}

impl ::std::fmt::Debug for CronScheduleView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for CronScheduleView<'_> {
  fn default() -> CronScheduleView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, CronSchedule>> for CronScheduleView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, CronSchedule>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> CronScheduleView<'msg> {

  pub fn to_owned(&self) -> CronSchedule {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // expr: optional string
  pub fn has_expr(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn expr_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.expr(), self.has_expr())
  }
  pub fn expr(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

  // timezone: optional string
  pub fn has_timezone(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn timezone_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.timezone(), self.has_timezone())
  }
  pub fn timezone(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        1, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

}

// SAFETY:
// - `CronScheduleView` is `Sync` because it does not support mutation.
unsafe impl Sync for CronScheduleView<'_> {}

// SAFETY:
// - `CronScheduleView` is `Send` because while its alive a `CronScheduleMut` cannot.
// - `CronScheduleView` does not use thread-local data.
unsafe impl Send for CronScheduleView<'_> {}

impl<'msg> ::protobuf::AsView for CronScheduleView<'msg> {
  type Proxied = CronSchedule;
  fn as_view(&self) -> ::protobuf::View<'msg, CronSchedule> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for CronScheduleView<'msg> {
  fn into_view<'shorter>(self) -> CronScheduleView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<CronSchedule> for CronScheduleView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> CronSchedule {
    let mut dst = CronSchedule::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<CronSchedule> for CronScheduleMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> CronSchedule {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for CronSchedule {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for CronScheduleView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for CronScheduleMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct CronScheduleMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, CronSchedule>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for CronScheduleMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for CronScheduleMut<'msg> {
  type Message = CronSchedule;
}

impl ::std::fmt::Debug for CronScheduleMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, CronSchedule>> for CronScheduleMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, CronSchedule>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> CronScheduleMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, CronSchedule> {
    self.inner
  }

  pub fn to_owned(&self) -> CronSchedule {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // expr: optional string
  pub fn has_expr(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_expr(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn expr_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.expr(), self.has_expr())
  }
  pub fn expr(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_expr(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // timezone: optional string
  pub fn has_timezone(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_timezone(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn timezone_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.timezone(), self.has_timezone())
  }
  pub fn timezone(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        1, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_timezone(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val);
    }
  }

}

// SAFETY:
// - `CronScheduleMut` does not perform any shared mutation.
unsafe impl Send for CronScheduleMut<'_> {}

// SAFETY:
// - `CronScheduleMut` does not perform any shared mutation.
unsafe impl Sync for CronScheduleMut<'_> {}

impl<'msg> ::protobuf::AsView for CronScheduleMut<'msg> {
  type Proxied = CronSchedule;
  fn as_view(&self) -> ::protobuf::View<'_, CronSchedule> {
    CronScheduleView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for CronScheduleMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, CronSchedule>
  where
      'msg: 'shorter {
    CronScheduleView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for CronScheduleMut<'msg> {
  type MutProxied = CronSchedule;
  fn as_mut(&mut self) -> CronScheduleMut<'msg> {
    CronScheduleMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for CronScheduleMut<'msg> {
  fn into_mut<'shorter>(self) -> CronScheduleMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl CronSchedule {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, CronSchedule> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> CronScheduleView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> CronScheduleMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // expr: optional string
  pub fn has_expr(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_expr(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn expr_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.expr(), self.has_expr())
  }
  pub fn expr(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_expr(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // timezone: optional string
  pub fn has_timezone(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_timezone(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn timezone_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.timezone(), self.has_timezone())
  }
  pub fn timezone(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        1, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_timezone(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val);
    }
  }

}  // impl CronSchedule

impl ::std::ops::Drop for CronSchedule {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for CronSchedule {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for CronSchedule {
  type Proxied = Self;
  fn as_view(&self) -> CronScheduleView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for CronSchedule {
  type MutProxied = Self;
  fn as_mut(&mut self) -> CronScheduleMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for CronSchedule {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__CronSchedule_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$M11");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__CronSchedule_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__CronSchedule_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for CronSchedule {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for CronSchedule {
  type Msg = CronSchedule;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<CronSchedule> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for CronSchedule {
  type Msg = CronSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<CronSchedule> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for CronScheduleMut<'_> {
  type Msg = CronSchedule;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<CronSchedule> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for CronScheduleMut<'_> {
  type Msg = CronSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<CronSchedule> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for CronScheduleView<'_> {
  type Msg = CronSchedule;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<CronSchedule> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for CronScheduleMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobDelivery_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobDelivery {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobDelivery>
}

impl ::protobuf::Message for JobDelivery {}

impl ::std::default::Default for JobDelivery {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobDelivery {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobDelivery` is `Sync` because it does not implement interior mutability.
//    Neither does `JobDeliveryMut`.
unsafe impl Sync for JobDelivery {}

// SAFETY:
// - `JobDelivery` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobDelivery {}

impl ::protobuf::Proxied for JobDelivery {
  type View<'msg> = JobDeliveryView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobDelivery {}

impl ::protobuf::MutProxied for JobDelivery {
  type Mut<'msg> = JobDeliveryMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobDeliveryView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobDelivery>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobDeliveryView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobDeliveryView<'msg> {
  type Message = JobDelivery;
}

impl ::std::fmt::Debug for JobDeliveryView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobDeliveryView<'_> {
  fn default() -> JobDeliveryView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobDelivery>> for JobDeliveryView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobDelivery>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobDeliveryView<'msg> {

  pub fn to_owned(&self) -> JobDelivery {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // nats_event: optional message trogon.cron.jobs.v1.NatsEventDelivery
  pub fn has_nats_event(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn nats_event_opt(self) -> ::protobuf::Optional<super::NatsEventDeliveryView<'msg>> {
        ::protobuf::Optional::new(self.nats_event(), self.has_nats_event())
  }
  pub fn nats_event(self) -> super::NatsEventDeliveryView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::NatsEventDeliveryView::default())
  }

  pub fn kind(self) -> super::job_delivery::KindOneof<'msg> {
    match self.kind_case() {
      super::job_delivery::KindCase::NatsEvent =>
          super::job_delivery::KindOneof::NatsEvent(self.nats_event()),
      _ => super::job_delivery::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(self) -> super::job_delivery::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_delivery::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}

// SAFETY:
// - `JobDeliveryView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobDeliveryView<'_> {}

// SAFETY:
// - `JobDeliveryView` is `Send` because while its alive a `JobDeliveryMut` cannot.
// - `JobDeliveryView` does not use thread-local data.
unsafe impl Send for JobDeliveryView<'_> {}

impl<'msg> ::protobuf::AsView for JobDeliveryView<'msg> {
  type Proxied = JobDelivery;
  fn as_view(&self) -> ::protobuf::View<'msg, JobDelivery> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobDeliveryView<'msg> {
  fn into_view<'shorter>(self) -> JobDeliveryView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobDelivery> for JobDeliveryView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobDelivery {
    let mut dst = JobDelivery::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobDelivery> for JobDeliveryMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobDelivery {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobDelivery {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobDeliveryView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobDeliveryMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobDeliveryMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobDelivery>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobDeliveryMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobDeliveryMut<'msg> {
  type Message = JobDelivery;
}

impl ::std::fmt::Debug for JobDeliveryMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobDelivery>> for JobDeliveryMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobDelivery>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobDeliveryMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobDelivery> {
    self.inner
  }

  pub fn to_owned(&self) -> JobDelivery {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // nats_event: optional message trogon.cron.jobs.v1.NatsEventDelivery
  pub fn has_nats_event(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_nats_event(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn nats_event_opt(&self) -> ::protobuf::Optional<super::NatsEventDeliveryView<'_>> {
        ::protobuf::Optional::new(self.nats_event(), self.has_nats_event())
  }
  pub fn nats_event(&self) -> super::NatsEventDeliveryView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::NatsEventDeliveryView::default())
  }
  pub fn nats_event_mut(&mut self) -> super::NatsEventDeliveryMut<'_> {
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
  pub fn set_nats_event(&mut self,
    val: impl ::protobuf::IntoProxied<super::NatsEventDelivery>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val
      );
    }
  }

  pub fn kind(&self) -> super::job_delivery::KindOneof<'_> {
    match &self.kind_case() {
      super::job_delivery::KindCase::NatsEvent =>
          super::job_delivery::KindOneof::NatsEvent(self.nats_event()),
      _ => super::job_delivery::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(&self) -> super::job_delivery::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_delivery::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}

// SAFETY:
// - `JobDeliveryMut` does not perform any shared mutation.
unsafe impl Send for JobDeliveryMut<'_> {}

// SAFETY:
// - `JobDeliveryMut` does not perform any shared mutation.
unsafe impl Sync for JobDeliveryMut<'_> {}

impl<'msg> ::protobuf::AsView for JobDeliveryMut<'msg> {
  type Proxied = JobDelivery;
  fn as_view(&self) -> ::protobuf::View<'_, JobDelivery> {
    JobDeliveryView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobDeliveryMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobDelivery>
  where
      'msg: 'shorter {
    JobDeliveryView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobDeliveryMut<'msg> {
  type MutProxied = JobDelivery;
  fn as_mut(&mut self) -> JobDeliveryMut<'msg> {
    JobDeliveryMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobDeliveryMut<'msg> {
  fn into_mut<'shorter>(self) -> JobDeliveryMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobDelivery {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobDelivery> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobDeliveryView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobDeliveryMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // nats_event: optional message trogon.cron.jobs.v1.NatsEventDelivery
  pub fn has_nats_event(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_nats_event(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn nats_event_opt(&self) -> ::protobuf::Optional<super::NatsEventDeliveryView<'_>> {
        ::protobuf::Optional::new(self.nats_event(), self.has_nats_event())
  }
  pub fn nats_event(&self) -> super::NatsEventDeliveryView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::NatsEventDeliveryView::default())
  }
  pub fn nats_event_mut(&mut self) -> super::NatsEventDeliveryMut<'_> {
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
  pub fn set_nats_event(&mut self,
    val: impl ::protobuf::IntoProxied<super::NatsEventDelivery>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val
      );
    }
  }

  pub fn kind(&self) -> super::job_delivery::KindOneof<'_> {
    match &self.kind_case() {
      super::job_delivery::KindCase::NatsEvent =>
          super::job_delivery::KindOneof::NatsEvent(self.nats_event()),
      _ => super::job_delivery::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(&self) -> super::job_delivery::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_delivery::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}  // impl JobDelivery

impl ::std::ops::Drop for JobDelivery {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobDelivery {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobDelivery {
  type Proxied = Self;
  fn as_view(&self) -> JobDeliveryView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobDelivery {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobDeliveryMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobDelivery {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobDelivery_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$3^!");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobDelivery_msg_init.0, &[<super::NatsEventDelivery as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            ], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobDelivery_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobDelivery {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobDelivery {
  type Msg = JobDelivery;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDelivery> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobDelivery {
  type Msg = JobDelivery;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDelivery> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobDeliveryMut<'_> {
  type Msg = JobDelivery;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDelivery> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobDeliveryMut<'_> {
  type Msg = JobDelivery;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDelivery> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobDeliveryView<'_> {
  type Msg = JobDelivery;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobDelivery> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobDeliveryMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

pub mod job_delivery {

#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
#[repr(u32)]
pub enum KindOneof<'msg> {
  NatsEvent(::protobuf::View<'msg, super::super::NatsEventDelivery>) = 1,

  not_set(std::marker::PhantomData<&'msg ()>) = 0
}
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(dead_code)]
pub enum KindCase {
  NatsEvent = 1,

  not_set = 0
}

impl KindCase {
  #[allow(dead_code)]
  pub(crate) fn try_from(v: u32) -> ::std::option::Option<KindCase> {
    match v {
      0 => Some(KindCase::not_set),
      1 => Some(KindCase::NatsEvent),
      _ => None
    }
  }
}
}  // pub mod job_delivery


// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__NatsEventDelivery_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct NatsEventDelivery {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<NatsEventDelivery>
}

impl ::protobuf::Message for NatsEventDelivery {}

impl ::std::default::Default for NatsEventDelivery {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for NatsEventDelivery {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `NatsEventDelivery` is `Sync` because it does not implement interior mutability.
//    Neither does `NatsEventDeliveryMut`.
unsafe impl Sync for NatsEventDelivery {}

// SAFETY:
// - `NatsEventDelivery` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for NatsEventDelivery {}

impl ::protobuf::Proxied for NatsEventDelivery {
  type View<'msg> = NatsEventDeliveryView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for NatsEventDelivery {}

impl ::protobuf::MutProxied for NatsEventDelivery {
  type Mut<'msg> = NatsEventDeliveryMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct NatsEventDeliveryView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, NatsEventDelivery>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for NatsEventDeliveryView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for NatsEventDeliveryView<'msg> {
  type Message = NatsEventDelivery;
}

impl ::std::fmt::Debug for NatsEventDeliveryView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for NatsEventDeliveryView<'_> {
  fn default() -> NatsEventDeliveryView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, NatsEventDelivery>> for NatsEventDeliveryView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, NatsEventDelivery>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> NatsEventDeliveryView<'msg> {

  pub fn to_owned(&self) -> NatsEventDelivery {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // route: optional string
  pub fn has_route(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn route_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.route(), self.has_route())
  }
  pub fn route(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

  // ttl_sec: optional uint64
  pub fn has_ttl_sec(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn ttl_sec_opt(self) -> ::protobuf::Optional<u64> {
        ::protobuf::Optional::new(self.ttl_sec(), self.has_ttl_sec())
  }
  pub fn ttl_sec(self) -> u64 {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_u64_at_index(
        1, (0u64).into()
      ).try_into().unwrap()
    }
  }

  // source: optional message trogon.cron.jobs.v1.JobSamplingSource
  pub fn has_source(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn source_opt(self) -> ::protobuf::Optional<super::JobSamplingSourceView<'msg>> {
        ::protobuf::Optional::new(self.source(), self.has_source())
  }
  pub fn source(self) -> super::JobSamplingSourceView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobSamplingSourceView::default())
  }

}

// SAFETY:
// - `NatsEventDeliveryView` is `Sync` because it does not support mutation.
unsafe impl Sync for NatsEventDeliveryView<'_> {}

// SAFETY:
// - `NatsEventDeliveryView` is `Send` because while its alive a `NatsEventDeliveryMut` cannot.
// - `NatsEventDeliveryView` does not use thread-local data.
unsafe impl Send for NatsEventDeliveryView<'_> {}

impl<'msg> ::protobuf::AsView for NatsEventDeliveryView<'msg> {
  type Proxied = NatsEventDelivery;
  fn as_view(&self) -> ::protobuf::View<'msg, NatsEventDelivery> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for NatsEventDeliveryView<'msg> {
  fn into_view<'shorter>(self) -> NatsEventDeliveryView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<NatsEventDelivery> for NatsEventDeliveryView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> NatsEventDelivery {
    let mut dst = NatsEventDelivery::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<NatsEventDelivery> for NatsEventDeliveryMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> NatsEventDelivery {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for NatsEventDelivery {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for NatsEventDeliveryView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for NatsEventDeliveryMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct NatsEventDeliveryMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, NatsEventDelivery>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for NatsEventDeliveryMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for NatsEventDeliveryMut<'msg> {
  type Message = NatsEventDelivery;
}

impl ::std::fmt::Debug for NatsEventDeliveryMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, NatsEventDelivery>> for NatsEventDeliveryMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, NatsEventDelivery>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> NatsEventDeliveryMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, NatsEventDelivery> {
    self.inner
  }

  pub fn to_owned(&self) -> NatsEventDelivery {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // route: optional string
  pub fn has_route(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_route(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn route_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.route(), self.has_route())
  }
  pub fn route(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_route(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // ttl_sec: optional uint64
  pub fn has_ttl_sec(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_ttl_sec(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn ttl_sec_opt(&self) -> ::protobuf::Optional<u64> {
        ::protobuf::Optional::new(self.ttl_sec(), self.has_ttl_sec())
  }
  pub fn ttl_sec(&self) -> u64 {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_u64_at_index(
        1, (0u64).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_ttl_sec(&mut self, val: u64) {
    unsafe {
      // TODO: b/361751487: This .into() is only here
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      //perfectly.
      self.inner.ptr_mut().set_base_field_u64_at_index(
        1, val.into()
      )
    }
  }

  // source: optional message trogon.cron.jobs.v1.JobSamplingSource
  pub fn has_source(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn clear_source(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        2
      );
    }
  }
  pub fn source_opt(&self) -> ::protobuf::Optional<super::JobSamplingSourceView<'_>> {
        ::protobuf::Optional::new(self.source(), self.has_source())
  }
  pub fn source(&self) -> super::JobSamplingSourceView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobSamplingSourceView::default())
  }
  pub fn source_mut(&mut self) -> super::JobSamplingSourceMut<'_> {
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
  pub fn set_source(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobSamplingSource>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        2,
        val
      );
    }
  }

}

// SAFETY:
// - `NatsEventDeliveryMut` does not perform any shared mutation.
unsafe impl Send for NatsEventDeliveryMut<'_> {}

// SAFETY:
// - `NatsEventDeliveryMut` does not perform any shared mutation.
unsafe impl Sync for NatsEventDeliveryMut<'_> {}

impl<'msg> ::protobuf::AsView for NatsEventDeliveryMut<'msg> {
  type Proxied = NatsEventDelivery;
  fn as_view(&self) -> ::protobuf::View<'_, NatsEventDelivery> {
    NatsEventDeliveryView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for NatsEventDeliveryMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, NatsEventDelivery>
  where
      'msg: 'shorter {
    NatsEventDeliveryView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for NatsEventDeliveryMut<'msg> {
  type MutProxied = NatsEventDelivery;
  fn as_mut(&mut self) -> NatsEventDeliveryMut<'msg> {
    NatsEventDeliveryMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for NatsEventDeliveryMut<'msg> {
  fn into_mut<'shorter>(self) -> NatsEventDeliveryMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl NatsEventDelivery {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, NatsEventDelivery> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> NatsEventDeliveryView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> NatsEventDeliveryMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // route: optional string
  pub fn has_route(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_route(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn route_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.route(), self.has_route())
  }
  pub fn route(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_route(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // ttl_sec: optional uint64
  pub fn has_ttl_sec(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_ttl_sec(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn ttl_sec_opt(&self) -> ::protobuf::Optional<u64> {
        ::protobuf::Optional::new(self.ttl_sec(), self.has_ttl_sec())
  }
  pub fn ttl_sec(&self) -> u64 {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_u64_at_index(
        1, (0u64).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_ttl_sec(&mut self, val: u64) {
    unsafe {
      // TODO: b/361751487: This .into() is only here
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      //perfectly.
      self.inner.ptr_mut().set_base_field_u64_at_index(
        1, val.into()
      )
    }
  }

  // source: optional message trogon.cron.jobs.v1.JobSamplingSource
  pub fn has_source(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(2)
    }
  }
  pub fn clear_source(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        2
      );
    }
  }
  pub fn source_opt(&self) -> ::protobuf::Optional<super::JobSamplingSourceView<'_>> {
        ::protobuf::Optional::new(self.source(), self.has_source())
  }
  pub fn source(&self) -> super::JobSamplingSourceView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(2)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobSamplingSourceView::default())
  }
  pub fn source_mut(&mut self) -> super::JobSamplingSourceMut<'_> {
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
  pub fn set_source(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobSamplingSource>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        2,
        val
      );
    }
  }

}  // impl NatsEventDelivery

impl ::std::ops::Drop for NatsEventDelivery {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for NatsEventDelivery {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for NatsEventDelivery {
  type Proxied = Self;
  fn as_view(&self) -> NatsEventDeliveryView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for NatsEventDelivery {
  type MutProxied = Self;
  fn as_mut(&mut self) -> NatsEventDeliveryMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for NatsEventDelivery {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__NatsEventDelivery_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$1T,3");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__NatsEventDelivery_msg_init.0, &[<super::JobSamplingSource as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            ], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__NatsEventDelivery_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for NatsEventDelivery {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for NatsEventDelivery {
  type Msg = NatsEventDelivery;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<NatsEventDelivery> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for NatsEventDelivery {
  type Msg = NatsEventDelivery;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<NatsEventDelivery> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for NatsEventDeliveryMut<'_> {
  type Msg = NatsEventDelivery;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<NatsEventDelivery> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for NatsEventDeliveryMut<'_> {
  type Msg = NatsEventDelivery;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<NatsEventDelivery> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for NatsEventDeliveryView<'_> {
  type Msg = NatsEventDelivery;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<NatsEventDelivery> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for NatsEventDeliveryMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobSamplingSource_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobSamplingSource {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobSamplingSource>
}

impl ::protobuf::Message for JobSamplingSource {}

impl ::std::default::Default for JobSamplingSource {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobSamplingSource {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobSamplingSource` is `Sync` because it does not implement interior mutability.
//    Neither does `JobSamplingSourceMut`.
unsafe impl Sync for JobSamplingSource {}

// SAFETY:
// - `JobSamplingSource` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobSamplingSource {}

impl ::protobuf::Proxied for JobSamplingSource {
  type View<'msg> = JobSamplingSourceView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobSamplingSource {}

impl ::protobuf::MutProxied for JobSamplingSource {
  type Mut<'msg> = JobSamplingSourceMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobSamplingSourceView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobSamplingSource>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobSamplingSourceView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobSamplingSourceView<'msg> {
  type Message = JobSamplingSource;
}

impl ::std::fmt::Debug for JobSamplingSourceView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobSamplingSourceView<'_> {
  fn default() -> JobSamplingSourceView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobSamplingSource>> for JobSamplingSourceView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobSamplingSource>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobSamplingSourceView<'msg> {

  pub fn to_owned(&self) -> JobSamplingSource {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // latest_from_subject: optional message trogon.cron.jobs.v1.LatestFromSubjectSampling
  pub fn has_latest_from_subject(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn latest_from_subject_opt(self) -> ::protobuf::Optional<super::LatestFromSubjectSamplingView<'msg>> {
        ::protobuf::Optional::new(self.latest_from_subject(), self.has_latest_from_subject())
  }
  pub fn latest_from_subject(self) -> super::LatestFromSubjectSamplingView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::LatestFromSubjectSamplingView::default())
  }

  pub fn kind(self) -> super::job_sampling_source::KindOneof<'msg> {
    match self.kind_case() {
      super::job_sampling_source::KindCase::LatestFromSubject =>
          super::job_sampling_source::KindOneof::LatestFromSubject(self.latest_from_subject()),
      _ => super::job_sampling_source::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(self) -> super::job_sampling_source::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_sampling_source::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}

// SAFETY:
// - `JobSamplingSourceView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobSamplingSourceView<'_> {}

// SAFETY:
// - `JobSamplingSourceView` is `Send` because while its alive a `JobSamplingSourceMut` cannot.
// - `JobSamplingSourceView` does not use thread-local data.
unsafe impl Send for JobSamplingSourceView<'_> {}

impl<'msg> ::protobuf::AsView for JobSamplingSourceView<'msg> {
  type Proxied = JobSamplingSource;
  fn as_view(&self) -> ::protobuf::View<'msg, JobSamplingSource> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobSamplingSourceView<'msg> {
  fn into_view<'shorter>(self) -> JobSamplingSourceView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobSamplingSource> for JobSamplingSourceView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobSamplingSource {
    let mut dst = JobSamplingSource::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobSamplingSource> for JobSamplingSourceMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobSamplingSource {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobSamplingSource {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobSamplingSourceView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobSamplingSourceMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobSamplingSourceMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobSamplingSource>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobSamplingSourceMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobSamplingSourceMut<'msg> {
  type Message = JobSamplingSource;
}

impl ::std::fmt::Debug for JobSamplingSourceMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobSamplingSource>> for JobSamplingSourceMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobSamplingSource>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobSamplingSourceMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobSamplingSource> {
    self.inner
  }

  pub fn to_owned(&self) -> JobSamplingSource {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // latest_from_subject: optional message trogon.cron.jobs.v1.LatestFromSubjectSampling
  pub fn has_latest_from_subject(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_latest_from_subject(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn latest_from_subject_opt(&self) -> ::protobuf::Optional<super::LatestFromSubjectSamplingView<'_>> {
        ::protobuf::Optional::new(self.latest_from_subject(), self.has_latest_from_subject())
  }
  pub fn latest_from_subject(&self) -> super::LatestFromSubjectSamplingView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::LatestFromSubjectSamplingView::default())
  }
  pub fn latest_from_subject_mut(&mut self) -> super::LatestFromSubjectSamplingMut<'_> {
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
  pub fn set_latest_from_subject(&mut self,
    val: impl ::protobuf::IntoProxied<super::LatestFromSubjectSampling>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val
      );
    }
  }

  pub fn kind(&self) -> super::job_sampling_source::KindOneof<'_> {
    match &self.kind_case() {
      super::job_sampling_source::KindCase::LatestFromSubject =>
          super::job_sampling_source::KindOneof::LatestFromSubject(self.latest_from_subject()),
      _ => super::job_sampling_source::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(&self) -> super::job_sampling_source::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_sampling_source::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}

// SAFETY:
// - `JobSamplingSourceMut` does not perform any shared mutation.
unsafe impl Send for JobSamplingSourceMut<'_> {}

// SAFETY:
// - `JobSamplingSourceMut` does not perform any shared mutation.
unsafe impl Sync for JobSamplingSourceMut<'_> {}

impl<'msg> ::protobuf::AsView for JobSamplingSourceMut<'msg> {
  type Proxied = JobSamplingSource;
  fn as_view(&self) -> ::protobuf::View<'_, JobSamplingSource> {
    JobSamplingSourceView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobSamplingSourceMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobSamplingSource>
  where
      'msg: 'shorter {
    JobSamplingSourceView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobSamplingSourceMut<'msg> {
  type MutProxied = JobSamplingSource;
  fn as_mut(&mut self) -> JobSamplingSourceMut<'msg> {
    JobSamplingSourceMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobSamplingSourceMut<'msg> {
  fn into_mut<'shorter>(self) -> JobSamplingSourceMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobSamplingSource {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobSamplingSource> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobSamplingSourceView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobSamplingSourceMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // latest_from_subject: optional message trogon.cron.jobs.v1.LatestFromSubjectSampling
  pub fn has_latest_from_subject(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_latest_from_subject(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn latest_from_subject_opt(&self) -> ::protobuf::Optional<super::LatestFromSubjectSamplingView<'_>> {
        ::protobuf::Optional::new(self.latest_from_subject(), self.has_latest_from_subject())
  }
  pub fn latest_from_subject(&self) -> super::LatestFromSubjectSamplingView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::LatestFromSubjectSamplingView::default())
  }
  pub fn latest_from_subject_mut(&mut self) -> super::LatestFromSubjectSamplingMut<'_> {
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
  pub fn set_latest_from_subject(&mut self,
    val: impl ::protobuf::IntoProxied<super::LatestFromSubjectSampling>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val
      );
    }
  }

  pub fn kind(&self) -> super::job_sampling_source::KindOneof<'_> {
    match &self.kind_case() {
      super::job_sampling_source::KindCase::LatestFromSubject =>
          super::job_sampling_source::KindOneof::LatestFromSubject(self.latest_from_subject()),
      _ => super::job_sampling_source::KindOneof::not_set(std::marker::PhantomData)
    }
  }

  pub fn kind_case(&self) -> super::job_sampling_source::KindCase {
    unsafe {
      let field_num = <Self as ::protobuf::__internal::runtime::UpbGetMessagePtr>::get_ptr(
          &self, ::protobuf::__internal::Private)
          .which_oneof_field_number_by_index(0);
      super::job_sampling_source::KindCase::try_from(field_num).unwrap_unchecked()
    }
  }
}  // impl JobSamplingSource

impl ::std::ops::Drop for JobSamplingSource {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobSamplingSource {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobSamplingSource {
  type Proxied = Self;
  fn as_view(&self) -> JobSamplingSourceView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobSamplingSource {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobSamplingSourceMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobSamplingSource {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobSamplingSource_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$3^!");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobSamplingSource_msg_init.0, &[<super::LatestFromSubjectSampling as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            ], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobSamplingSource_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobSamplingSource {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobSamplingSource {
  type Msg = JobSamplingSource;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSamplingSource> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobSamplingSource {
  type Msg = JobSamplingSource;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSamplingSource> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobSamplingSourceMut<'_> {
  type Msg = JobSamplingSource;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSamplingSource> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobSamplingSourceMut<'_> {
  type Msg = JobSamplingSource;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSamplingSource> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobSamplingSourceView<'_> {
  type Msg = JobSamplingSource;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobSamplingSource> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobSamplingSourceMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

pub mod job_sampling_source {

#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
#[repr(u32)]
pub enum KindOneof<'msg> {
  LatestFromSubject(::protobuf::View<'msg, super::super::LatestFromSubjectSampling>) = 1,

  not_set(std::marker::PhantomData<&'msg ()>) = 0
}
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(dead_code)]
pub enum KindCase {
  LatestFromSubject = 1,

  not_set = 0
}

impl KindCase {
  #[allow(dead_code)]
  pub(crate) fn try_from(v: u32) -> ::std::option::Option<KindCase> {
    match v {
      0 => Some(KindCase::not_set),
      1 => Some(KindCase::LatestFromSubject),
      _ => None
    }
  }
}
}  // pub mod job_sampling_source


// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__LatestFromSubjectSampling_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct LatestFromSubjectSampling {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<LatestFromSubjectSampling>
}

impl ::protobuf::Message for LatestFromSubjectSampling {}

impl ::std::default::Default for LatestFromSubjectSampling {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for LatestFromSubjectSampling {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `LatestFromSubjectSampling` is `Sync` because it does not implement interior mutability.
//    Neither does `LatestFromSubjectSamplingMut`.
unsafe impl Sync for LatestFromSubjectSampling {}

// SAFETY:
// - `LatestFromSubjectSampling` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for LatestFromSubjectSampling {}

impl ::protobuf::Proxied for LatestFromSubjectSampling {
  type View<'msg> = LatestFromSubjectSamplingView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for LatestFromSubjectSampling {}

impl ::protobuf::MutProxied for LatestFromSubjectSampling {
  type Mut<'msg> = LatestFromSubjectSamplingMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct LatestFromSubjectSamplingView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, LatestFromSubjectSampling>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for LatestFromSubjectSamplingView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for LatestFromSubjectSamplingView<'msg> {
  type Message = LatestFromSubjectSampling;
}

impl ::std::fmt::Debug for LatestFromSubjectSamplingView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for LatestFromSubjectSamplingView<'_> {
  fn default() -> LatestFromSubjectSamplingView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, LatestFromSubjectSampling>> for LatestFromSubjectSamplingView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, LatestFromSubjectSampling>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> LatestFromSubjectSamplingView<'msg> {

  pub fn to_owned(&self) -> LatestFromSubjectSampling {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // subject: optional string
  pub fn has_subject(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn subject_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.subject(), self.has_subject())
  }
  pub fn subject(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

}

// SAFETY:
// - `LatestFromSubjectSamplingView` is `Sync` because it does not support mutation.
unsafe impl Sync for LatestFromSubjectSamplingView<'_> {}

// SAFETY:
// - `LatestFromSubjectSamplingView` is `Send` because while its alive a `LatestFromSubjectSamplingMut` cannot.
// - `LatestFromSubjectSamplingView` does not use thread-local data.
unsafe impl Send for LatestFromSubjectSamplingView<'_> {}

impl<'msg> ::protobuf::AsView for LatestFromSubjectSamplingView<'msg> {
  type Proxied = LatestFromSubjectSampling;
  fn as_view(&self) -> ::protobuf::View<'msg, LatestFromSubjectSampling> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for LatestFromSubjectSamplingView<'msg> {
  fn into_view<'shorter>(self) -> LatestFromSubjectSamplingView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<LatestFromSubjectSampling> for LatestFromSubjectSamplingView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> LatestFromSubjectSampling {
    let mut dst = LatestFromSubjectSampling::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<LatestFromSubjectSampling> for LatestFromSubjectSamplingMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> LatestFromSubjectSampling {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for LatestFromSubjectSampling {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for LatestFromSubjectSamplingView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for LatestFromSubjectSamplingMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct LatestFromSubjectSamplingMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, LatestFromSubjectSampling>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for LatestFromSubjectSamplingMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for LatestFromSubjectSamplingMut<'msg> {
  type Message = LatestFromSubjectSampling;
}

impl ::std::fmt::Debug for LatestFromSubjectSamplingMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, LatestFromSubjectSampling>> for LatestFromSubjectSamplingMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, LatestFromSubjectSampling>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> LatestFromSubjectSamplingMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, LatestFromSubjectSampling> {
    self.inner
  }

  pub fn to_owned(&self) -> LatestFromSubjectSampling {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // subject: optional string
  pub fn has_subject(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_subject(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn subject_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.subject(), self.has_subject())
  }
  pub fn subject(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_subject(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}

// SAFETY:
// - `LatestFromSubjectSamplingMut` does not perform any shared mutation.
unsafe impl Send for LatestFromSubjectSamplingMut<'_> {}

// SAFETY:
// - `LatestFromSubjectSamplingMut` does not perform any shared mutation.
unsafe impl Sync for LatestFromSubjectSamplingMut<'_> {}

impl<'msg> ::protobuf::AsView for LatestFromSubjectSamplingMut<'msg> {
  type Proxied = LatestFromSubjectSampling;
  fn as_view(&self) -> ::protobuf::View<'_, LatestFromSubjectSampling> {
    LatestFromSubjectSamplingView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for LatestFromSubjectSamplingMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, LatestFromSubjectSampling>
  where
      'msg: 'shorter {
    LatestFromSubjectSamplingView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for LatestFromSubjectSamplingMut<'msg> {
  type MutProxied = LatestFromSubjectSampling;
  fn as_mut(&mut self) -> LatestFromSubjectSamplingMut<'msg> {
    LatestFromSubjectSamplingMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for LatestFromSubjectSamplingMut<'msg> {
  fn into_mut<'shorter>(self) -> LatestFromSubjectSamplingMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl LatestFromSubjectSampling {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, LatestFromSubjectSampling> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> LatestFromSubjectSamplingView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> LatestFromSubjectSamplingMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // subject: optional string
  pub fn has_subject(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_subject(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn subject_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.subject(), self.has_subject())
  }
  pub fn subject(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_subject(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

}  // impl LatestFromSubjectSampling

impl ::std::ops::Drop for LatestFromSubjectSampling {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for LatestFromSubjectSampling {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for LatestFromSubjectSampling {
  type Proxied = Self;
  fn as_view(&self) -> LatestFromSubjectSamplingView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for LatestFromSubjectSampling {
  type MutProxied = Self;
  fn as_mut(&mut self) -> LatestFromSubjectSamplingMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for LatestFromSubjectSampling {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__LatestFromSubjectSampling_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$M1");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__LatestFromSubjectSampling_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__LatestFromSubjectSampling_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for LatestFromSubjectSampling {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for LatestFromSubjectSampling {
  type Msg = LatestFromSubjectSampling;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<LatestFromSubjectSampling> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for LatestFromSubjectSampling {
  type Msg = LatestFromSubjectSampling;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<LatestFromSubjectSampling> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for LatestFromSubjectSamplingMut<'_> {
  type Msg = LatestFromSubjectSampling;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<LatestFromSubjectSampling> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for LatestFromSubjectSamplingMut<'_> {
  type Msg = LatestFromSubjectSampling;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<LatestFromSubjectSampling> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for LatestFromSubjectSamplingView<'_> {
  type Msg = LatestFromSubjectSampling;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<LatestFromSubjectSampling> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for LatestFromSubjectSamplingMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__JobMessage_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct JobMessage {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<JobMessage>
}

impl ::protobuf::Message for JobMessage {}

impl ::std::default::Default for JobMessage {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for JobMessage {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `JobMessage` is `Sync` because it does not implement interior mutability.
//    Neither does `JobMessageMut`.
unsafe impl Sync for JobMessage {}

// SAFETY:
// - `JobMessage` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for JobMessage {}

impl ::protobuf::Proxied for JobMessage {
  type View<'msg> = JobMessageView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for JobMessage {}

impl ::protobuf::MutProxied for JobMessage {
  type Mut<'msg> = JobMessageMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct JobMessageView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobMessage>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobMessageView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for JobMessageView<'msg> {
  type Message = JobMessage;
}

impl ::std::fmt::Debug for JobMessageView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for JobMessageView<'_> {
  fn default() -> JobMessageView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, JobMessage>> for JobMessageView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, JobMessage>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobMessageView<'msg> {

  pub fn to_owned(&self) -> JobMessage {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // content: optional string
  pub fn has_content(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn content_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.content(), self.has_content())
  }
  pub fn content(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

  // headers: repeated message trogon.cron.jobs.v1.Header
  pub fn headers(self) -> ::protobuf::RepeatedView<'msg, super::Header> {
    unsafe {
      self.inner.ptr().get_array_at_index(
        1
      )
    }.map_or_else(
        ::protobuf::__internal::runtime::empty_array::<super::Header>,
        |raw| unsafe {
          ::protobuf::RepeatedView::from_raw(::protobuf::__internal::Private, raw)
        }
      )
  }

}

// SAFETY:
// - `JobMessageView` is `Sync` because it does not support mutation.
unsafe impl Sync for JobMessageView<'_> {}

// SAFETY:
// - `JobMessageView` is `Send` because while its alive a `JobMessageMut` cannot.
// - `JobMessageView` does not use thread-local data.
unsafe impl Send for JobMessageView<'_> {}

impl<'msg> ::protobuf::AsView for JobMessageView<'msg> {
  type Proxied = JobMessage;
  fn as_view(&self) -> ::protobuf::View<'msg, JobMessage> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobMessageView<'msg> {
  fn into_view<'shorter>(self) -> JobMessageView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<JobMessage> for JobMessageView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobMessage {
    let mut dst = JobMessage::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<JobMessage> for JobMessageMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> JobMessage {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobMessage {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobMessageView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for JobMessageMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct JobMessageMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobMessage>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for JobMessageMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for JobMessageMut<'msg> {
  type Message = JobMessage;
}

impl ::std::fmt::Debug for JobMessageMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, JobMessage>> for JobMessageMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, JobMessage>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> JobMessageMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, JobMessage> {
    self.inner
  }

  pub fn to_owned(&self) -> JobMessage {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // content: optional string
  pub fn has_content(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_content(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn content_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.content(), self.has_content())
  }
  pub fn content(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_content(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // headers: repeated message trogon.cron.jobs.v1.Header
  pub fn headers(&self) -> ::protobuf::RepeatedView<'_, super::Header> {
    unsafe {
      self.inner.ptr().get_array_at_index(
        1
      )
    }.map_or_else(
        ::protobuf::__internal::runtime::empty_array::<super::Header>,
        |raw| unsafe {
          ::protobuf::RepeatedView::from_raw(::protobuf::__internal::Private, raw)
        }
      )
  }
  pub fn headers_mut(&mut self) -> ::protobuf::RepeatedMut<'_, super::Header> {
    unsafe {
      let raw_array = self.inner.ptr_mut().get_or_create_mutable_array_at_index(
        1,
        self.inner.arena()
      ).expect("alloc should not fail");
      ::protobuf::RepeatedMut::from_inner(
        ::protobuf::__internal::Private,
        ::protobuf::__internal::runtime::InnerRepeatedMut::new(
          raw_array, self.inner.arena(),
        ),
      )
    }
  }
  pub fn set_headers(&mut self, src: impl ::protobuf::IntoProxied<::protobuf::Repeated<super::Header>>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_repeated_field(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        src);
    }
  }

}

// SAFETY:
// - `JobMessageMut` does not perform any shared mutation.
unsafe impl Send for JobMessageMut<'_> {}

// SAFETY:
// - `JobMessageMut` does not perform any shared mutation.
unsafe impl Sync for JobMessageMut<'_> {}

impl<'msg> ::protobuf::AsView for JobMessageMut<'msg> {
  type Proxied = JobMessage;
  fn as_view(&self) -> ::protobuf::View<'_, JobMessage> {
    JobMessageView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobMessageMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, JobMessage>
  where
      'msg: 'shorter {
    JobMessageView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for JobMessageMut<'msg> {
  type MutProxied = JobMessage;
  fn as_mut(&mut self) -> JobMessageMut<'msg> {
    JobMessageMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for JobMessageMut<'msg> {
  fn into_mut<'shorter>(self) -> JobMessageMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl JobMessage {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, JobMessage> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> JobMessageView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> JobMessageMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // content: optional string
  pub fn has_content(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_content(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn content_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.content(), self.has_content())
  }
  pub fn content(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_content(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // headers: repeated message trogon.cron.jobs.v1.Header
  pub fn headers(&self) -> ::protobuf::RepeatedView<'_, super::Header> {
    unsafe {
      self.inner.ptr().get_array_at_index(
        1
      )
    }.map_or_else(
        ::protobuf::__internal::runtime::empty_array::<super::Header>,
        |raw| unsafe {
          ::protobuf::RepeatedView::from_raw(::protobuf::__internal::Private, raw)
        }
      )
  }
  pub fn headers_mut(&mut self) -> ::protobuf::RepeatedMut<'_, super::Header> {
    unsafe {
      let raw_array = self.inner.ptr_mut().get_or_create_mutable_array_at_index(
        1,
        self.inner.arena()
      ).expect("alloc should not fail");
      ::protobuf::RepeatedMut::from_inner(
        ::protobuf::__internal::Private,
        ::protobuf::__internal::runtime::InnerRepeatedMut::new(
          raw_array, self.inner.arena(),
        ),
      )
    }
  }
  pub fn set_headers(&mut self, src: impl ::protobuf::IntoProxied<::protobuf::Repeated<super::Header>>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_repeated_field(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        src);
    }
  }

}  // impl JobMessage

impl ::std::ops::Drop for JobMessage {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for JobMessage {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for JobMessage {
  type Proxied = Self;
  fn as_view(&self) -> JobMessageView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for JobMessage {
  type MutProxied = Self;
  fn as_mut(&mut self) -> JobMessageMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for JobMessage {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__JobMessage_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$1TG");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__JobMessage_msg_init.0, &[<super::Header as ::protobuf::__internal::runtime::AssociatedMiniTable>::mini_table(),
            ], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__JobMessage_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobMessage {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobMessage {
  type Msg = JobMessage;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobMessage> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobMessage {
  type Msg = JobMessage;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobMessage> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for JobMessageMut<'_> {
  type Msg = JobMessage;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobMessage> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobMessageMut<'_> {
  type Msg = JobMessage;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobMessage> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for JobMessageView<'_> {
  type Msg = JobMessage;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<JobMessage> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for JobMessageMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__v1__Header_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct Header {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<Header>
}

impl ::protobuf::Message for Header {}

impl ::std::default::Default for Header {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for Header {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `Header` is `Sync` because it does not implement interior mutability.
//    Neither does `HeaderMut`.
unsafe impl Sync for Header {}

// SAFETY:
// - `Header` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for Header {}

impl ::protobuf::Proxied for Header {
  type View<'msg> = HeaderView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for Header {}

impl ::protobuf::MutProxied for Header {
  type Mut<'msg> = HeaderMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct HeaderView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, Header>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for HeaderView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for HeaderView<'msg> {
  type Message = Header;
}

impl ::std::fmt::Debug for HeaderView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for HeaderView<'_> {
  fn default() -> HeaderView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, Header>> for HeaderView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, Header>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> HeaderView<'msg> {

  pub fn to_owned(&self) -> Header {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // name: optional string
  pub fn has_name(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn name_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.name(), self.has_name())
  }
  pub fn name(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

  // value: optional string
  pub fn has_value(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn value_opt(self) -> ::protobuf::Optional<&'msg ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.value(), self.has_value())
  }
  pub fn value(self) -> ::protobuf::View<'msg, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        1, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }

}

// SAFETY:
// - `HeaderView` is `Sync` because it does not support mutation.
unsafe impl Sync for HeaderView<'_> {}

// SAFETY:
// - `HeaderView` is `Send` because while its alive a `HeaderMut` cannot.
// - `HeaderView` does not use thread-local data.
unsafe impl Send for HeaderView<'_> {}

impl<'msg> ::protobuf::AsView for HeaderView<'msg> {
  type Proxied = Header;
  fn as_view(&self) -> ::protobuf::View<'msg, Header> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for HeaderView<'msg> {
  fn into_view<'shorter>(self) -> HeaderView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<Header> for HeaderView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> Header {
    let mut dst = Header::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<Header> for HeaderMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> Header {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for Header {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for HeaderView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for HeaderMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct HeaderMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, Header>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for HeaderMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for HeaderMut<'msg> {
  type Message = Header;
}

impl ::std::fmt::Debug for HeaderMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, Header>> for HeaderMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, Header>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> HeaderMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, Header> {
    self.inner
  }

  pub fn to_owned(&self) -> Header {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // name: optional string
  pub fn has_name(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_name(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn name_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.name(), self.has_name())
  }
  pub fn name(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_name(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // value: optional string
  pub fn has_value(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_value(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn value_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.value(), self.has_value())
  }
  pub fn value(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        1, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_value(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val);
    }
  }

}

// SAFETY:
// - `HeaderMut` does not perform any shared mutation.
unsafe impl Send for HeaderMut<'_> {}

// SAFETY:
// - `HeaderMut` does not perform any shared mutation.
unsafe impl Sync for HeaderMut<'_> {}

impl<'msg> ::protobuf::AsView for HeaderMut<'msg> {
  type Proxied = Header;
  fn as_view(&self) -> ::protobuf::View<'_, Header> {
    HeaderView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for HeaderMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, Header>
  where
      'msg: 'shorter {
    HeaderView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for HeaderMut<'msg> {
  type MutProxied = Header;
  fn as_mut(&mut self) -> HeaderMut<'msg> {
    HeaderMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for HeaderMut<'msg> {
  fn into_mut<'shorter>(self) -> HeaderMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl Header {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, Header> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> HeaderView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> HeaderMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // name: optional string
  pub fn has_name(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_name(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn name_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.name(), self.has_name())
  }
  pub fn name(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        0, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_name(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
        val);
    }
  }

  // value: optional string
  pub fn has_value(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(1)
    }
  }
  pub fn clear_value(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        1
      );
    }
  }
  pub fn value_opt(&self) -> ::protobuf::Optional<&'_ ::protobuf::ProtoStr> {
        ::protobuf::Optional::new(self.value(), self.has_value())
  }
  pub fn value(&self) -> ::protobuf::View<'_, ::protobuf::ProtoString> {
    let str_view = unsafe {
      self.inner.ptr().get_string_at_index(
        1, (b"").into()
      )
    };
    // SAFETY: The runtime doesn't require ProtoStr to be UTF-8.
    unsafe { ::protobuf::ProtoStr::from_utf8_unchecked(str_view.as_ref()) }
  }
  pub fn set_value(&mut self, val: impl ::protobuf::IntoProxied<::protobuf::ProtoString>) {
    unsafe {
      ::protobuf::__internal::runtime::message_set_string_field(
        ::protobuf::AsMut::as_mut(self).inner,
        1,
        val);
    }
  }

}  // impl Header

impl ::std::ops::Drop for Header {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for Header {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for Header {
  type Proxied = Self;
  fn as_view(&self) -> HeaderView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for Header {
  type MutProxied = Self;
  fn as_mut(&mut self) -> HeaderMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for Header {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__v1__Header_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$M11");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__v1__Header_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__v1__Header_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for Header {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for Header {
  type Msg = Header;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<Header> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for Header {
  type Msg = Header;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<Header> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for HeaderMut<'_> {
  type Msg = Header;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<Header> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for HeaderMut<'_> {
  type Msg = Header;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<Header> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for HeaderView<'_> {
  type Msg = Header;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<Header> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for HeaderMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobStatus(i32);

#[allow(non_upper_case_globals)]
impl JobStatus {
  pub const Unspecified: JobStatus = JobStatus(0);
  pub const Enabled: JobStatus = JobStatus(1);
  pub const Disabled: JobStatus = JobStatus(2);

  fn constant_name(&self) -> ::std::option::Option<&'static str> {
    #[allow(unreachable_patterns)] // In the case of aliases, just emit them all and let the first one match.
    Some(match self.0 {
      0 => "Unspecified",
      1 => "Enabled",
      2 => "Disabled",
      _ => return None
    })
  }
}

impl ::std::convert::From<JobStatus> for i32 {
  fn from(val: JobStatus) -> i32 {
    val.0
  }
}

impl ::std::convert::From<i32> for JobStatus {
  fn from(val: i32) -> JobStatus {
    Self(val)
  }
}

impl ::std::default::Default for JobStatus {
  fn default() -> Self {
    Self(0)
  }
}

impl ::std::fmt::Debug for JobStatus {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    if let Some(constant_name) = self.constant_name() {
      write!(f, "JobStatus::{}", constant_name)
    } else {
      write!(f, "JobStatus::from({})", self.0)
    }
  }
}

impl ::protobuf::IntoProxied<i32> for JobStatus {
  fn into_proxied(self, _: ::protobuf::__internal::Private) -> i32 {
    self.0
  }
}

impl ::protobuf::__internal::SealedInternal for JobStatus {}

impl ::protobuf::Proxied for JobStatus {
  type View<'a> = JobStatus;
}

impl ::protobuf::AsView for JobStatus {
  type Proxied = JobStatus;

  fn as_view(&self) -> JobStatus {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for JobStatus {
  fn into_view<'shorter>(self) -> JobStatus where 'msg: 'shorter {
    self
  }
}

// SAFETY: this is an enum type
unsafe impl ::protobuf::__internal::Enum for JobStatus {
  const NAME: &'static str = "JobStatus";

  fn is_known(value: i32) -> bool {
    matches!(value, 0|1|2)
  }
}

impl ::protobuf::__internal::runtime::EntityType for JobStatus {
    type Tag = ::protobuf::__internal::runtime::EnumTag;
}


