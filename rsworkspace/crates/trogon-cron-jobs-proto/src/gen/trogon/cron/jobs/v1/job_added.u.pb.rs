const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
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

  // job: optional message trogon.cron.jobs.v1.JobDetails
  pub fn has_job(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn job_opt(self) -> ::protobuf::Optional<super::JobDetailsView<'msg>> {
        ::protobuf::Optional::new(self.job(), self.has_job())
  }
  pub fn job(self) -> super::JobDetailsView<'msg> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
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

  // job: optional message trogon.cron.jobs.v1.JobDetails
  pub fn has_job(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_job(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn job_opt(&self) -> ::protobuf::Optional<super::JobDetailsView<'_>> {
        ::protobuf::Optional::new(self.job(), self.has_job())
  }
  pub fn job(&self) -> super::JobDetailsView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobDetailsView::default())
  }
  pub fn job_mut(&mut self) -> super::JobDetailsMut<'_> {
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
  pub fn set_job(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobDetails>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
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

  // job: optional message trogon.cron.jobs.v1.JobDetails
  pub fn has_job(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_job(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn job_opt(&self) -> ::protobuf::Optional<super::JobDetailsView<'_>> {
        ::protobuf::Optional::new(self.job(), self.has_job())
  }
  pub fn job(&self) -> super::JobDetailsView<'_> {
    let submsg = unsafe {
      self.inner.ptr().get_message_at_index(0)
    };
    submsg
        .map(|ptr| unsafe { ::protobuf::__internal::runtime::MessageViewInner::wrap(ptr).into() })
       .unwrap_or(super::JobDetailsView::default())
  }
  pub fn job_mut(&mut self) -> super::JobDetailsMut<'_> {
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
  pub fn set_job(&mut self,
    val: impl ::protobuf::IntoProxied<super::JobDetails>) {

    unsafe {
      ::protobuf::__internal::runtime::message_set_sub_message(
        ::protobuf::AsMut::as_mut(self).inner,
        0,
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
            ::protobuf::__internal::runtime::build_mini_table("$3");
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



