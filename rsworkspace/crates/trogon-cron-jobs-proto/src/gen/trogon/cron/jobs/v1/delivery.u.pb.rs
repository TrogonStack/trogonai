const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
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



