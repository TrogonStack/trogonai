const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
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



