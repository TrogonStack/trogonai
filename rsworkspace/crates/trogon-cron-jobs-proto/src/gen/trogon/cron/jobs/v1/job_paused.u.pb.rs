const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
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
            ::protobuf::__internal::runtime::build_mini_table("$");
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



