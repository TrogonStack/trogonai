const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
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
            ::protobuf::__internal::runtime::build_mini_table("$");
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



