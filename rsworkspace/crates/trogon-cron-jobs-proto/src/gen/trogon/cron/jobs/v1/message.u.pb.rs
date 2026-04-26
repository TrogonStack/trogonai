const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
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



