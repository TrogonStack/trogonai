const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
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
            ::protobuf::__internal::runtime::build_mini_table("$");
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



