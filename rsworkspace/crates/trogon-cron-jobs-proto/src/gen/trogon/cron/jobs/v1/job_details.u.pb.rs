const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
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


