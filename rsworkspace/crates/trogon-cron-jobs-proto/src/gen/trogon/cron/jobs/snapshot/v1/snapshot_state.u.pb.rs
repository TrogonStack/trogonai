const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__snapshot__v1__SnapshotState_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct SnapshotState {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<SnapshotState>
}

impl ::protobuf::Message for SnapshotState {}

impl ::std::default::Default for SnapshotState {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for SnapshotState {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `SnapshotState` is `Sync` because it does not implement interior mutability.
//    Neither does `SnapshotStateMut`.
unsafe impl Sync for SnapshotState {}

// SAFETY:
// - `SnapshotState` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for SnapshotState {}

impl ::protobuf::Proxied for SnapshotState {
  type View<'msg> = SnapshotStateView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for SnapshotState {}

impl ::protobuf::MutProxied for SnapshotState {
  type Mut<'msg> = SnapshotStateMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct SnapshotStateView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, SnapshotState>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for SnapshotStateView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for SnapshotStateView<'msg> {
  type Message = SnapshotState;
}

impl ::std::fmt::Debug for SnapshotStateView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for SnapshotStateView<'_> {
  fn default() -> SnapshotStateView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, SnapshotState>> for SnapshotStateView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, SnapshotState>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> SnapshotStateView<'msg> {

  pub fn to_owned(&self) -> SnapshotState {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // state: optional enum trogon.cron.jobs.snapshot.v1.SnapshotStateValue
  pub fn has_state(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn state_opt(self) -> ::protobuf::Optional<super::SnapshotStateValue> {
        ::protobuf::Optional::new(self.state(), self.has_state())
  }
  pub fn state(self) -> super::SnapshotStateValue {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::SnapshotStateValue::Unspecified).into()
      ).try_into().unwrap()
    }
  }

}

// SAFETY:
// - `SnapshotStateView` is `Sync` because it does not support mutation.
unsafe impl Sync for SnapshotStateView<'_> {}

// SAFETY:
// - `SnapshotStateView` is `Send` because while its alive a `SnapshotStateMut` cannot.
// - `SnapshotStateView` does not use thread-local data.
unsafe impl Send for SnapshotStateView<'_> {}

impl<'msg> ::protobuf::AsView for SnapshotStateView<'msg> {
  type Proxied = SnapshotState;
  fn as_view(&self) -> ::protobuf::View<'msg, SnapshotState> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for SnapshotStateView<'msg> {
  fn into_view<'shorter>(self) -> SnapshotStateView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<SnapshotState> for SnapshotStateView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> SnapshotState {
    let mut dst = SnapshotState::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<SnapshotState> for SnapshotStateMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> SnapshotState {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for SnapshotState {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for SnapshotStateView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for SnapshotStateMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct SnapshotStateMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, SnapshotState>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for SnapshotStateMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for SnapshotStateMut<'msg> {
  type Message = SnapshotState;
}

impl ::std::fmt::Debug for SnapshotStateMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, SnapshotState>> for SnapshotStateMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, SnapshotState>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> SnapshotStateMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, SnapshotState> {
    self.inner
  }

  pub fn to_owned(&self) -> SnapshotState {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // state: optional enum trogon.cron.jobs.snapshot.v1.SnapshotStateValue
  pub fn has_state(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_state(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn state_opt(&self) -> ::protobuf::Optional<super::SnapshotStateValue> {
        ::protobuf::Optional::new(self.state(), self.has_state())
  }
  pub fn state(&self) -> super::SnapshotStateValue {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::SnapshotStateValue::Unspecified).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_state(&mut self, val: super::SnapshotStateValue) {
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

}

// SAFETY:
// - `SnapshotStateMut` does not perform any shared mutation.
unsafe impl Send for SnapshotStateMut<'_> {}

// SAFETY:
// - `SnapshotStateMut` does not perform any shared mutation.
unsafe impl Sync for SnapshotStateMut<'_> {}

impl<'msg> ::protobuf::AsView for SnapshotStateMut<'msg> {
  type Proxied = SnapshotState;
  fn as_view(&self) -> ::protobuf::View<'_, SnapshotState> {
    SnapshotStateView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for SnapshotStateMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, SnapshotState>
  where
      'msg: 'shorter {
    SnapshotStateView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for SnapshotStateMut<'msg> {
  type MutProxied = SnapshotState;
  fn as_mut(&mut self) -> SnapshotStateMut<'msg> {
    SnapshotStateMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for SnapshotStateMut<'msg> {
  fn into_mut<'shorter>(self) -> SnapshotStateMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl SnapshotState {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, SnapshotState> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> SnapshotStateView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> SnapshotStateMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // state: optional enum trogon.cron.jobs.snapshot.v1.SnapshotStateValue
  pub fn has_state(&self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn clear_state(&mut self) {
    unsafe {
      self.inner.ptr().clear_field_at_index(
        0
      );
    }
  }
  pub fn state_opt(&self) -> ::protobuf::Optional<super::SnapshotStateValue> {
        ::protobuf::Optional::new(self.state(), self.has_state())
  }
  pub fn state(&self) -> super::SnapshotStateValue {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::SnapshotStateValue::Unspecified).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_state(&mut self, val: super::SnapshotStateValue) {
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

}  // impl SnapshotState

impl ::std::ops::Drop for SnapshotState {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for SnapshotState {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for SnapshotState {
  type Proxied = Self;
  fn as_view(&self) -> SnapshotStateView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for SnapshotState {
  type MutProxied = Self;
  fn as_mut(&mut self) -> SnapshotStateMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for SnapshotState {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__snapshot__v1__SnapshotState_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$.");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__snapshot__v1__SnapshotState_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__snapshot__v1__SnapshotState_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for SnapshotState {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for SnapshotState {
  type Msg = SnapshotState;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<SnapshotState> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for SnapshotState {
  type Msg = SnapshotState;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<SnapshotState> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for SnapshotStateMut<'_> {
  type Msg = SnapshotState;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<SnapshotState> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for SnapshotStateMut<'_> {
  type Msg = SnapshotState;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<SnapshotState> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for SnapshotStateView<'_> {
  type Msg = SnapshotState;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<SnapshotState> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for SnapshotStateMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SnapshotStateValue(i32);

#[allow(non_upper_case_globals)]
impl SnapshotStateValue {
  pub const Unspecified: SnapshotStateValue = SnapshotStateValue(0);
  pub const Missing: SnapshotStateValue = SnapshotStateValue(1);
  pub const PresentEnabled: SnapshotStateValue = SnapshotStateValue(2);
  pub const PresentDisabled: SnapshotStateValue = SnapshotStateValue(3);
  pub const Deleted: SnapshotStateValue = SnapshotStateValue(4);

  fn constant_name(&self) -> ::std::option::Option<&'static str> {
    #[allow(unreachable_patterns)] // In the case of aliases, just emit them all and let the first one match.
    Some(match self.0 {
      0 => "Unspecified",
      1 => "Missing",
      2 => "PresentEnabled",
      3 => "PresentDisabled",
      4 => "Deleted",
      _ => return None
    })
  }
}

impl ::std::convert::From<SnapshotStateValue> for i32 {
  fn from(val: SnapshotStateValue) -> i32 {
    val.0
  }
}

impl ::std::convert::From<i32> for SnapshotStateValue {
  fn from(val: i32) -> SnapshotStateValue {
    Self(val)
  }
}

impl ::std::default::Default for SnapshotStateValue {
  fn default() -> Self {
    Self(0)
  }
}

impl ::std::fmt::Debug for SnapshotStateValue {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    if let Some(constant_name) = self.constant_name() {
      write!(f, "SnapshotStateValue::{}", constant_name)
    } else {
      write!(f, "SnapshotStateValue::from({})", self.0)
    }
  }
}

impl ::protobuf::IntoProxied<i32> for SnapshotStateValue {
  fn into_proxied(self, _: ::protobuf::__internal::Private) -> i32 {
    self.0
  }
}

impl ::protobuf::__internal::SealedInternal for SnapshotStateValue {}

impl ::protobuf::Proxied for SnapshotStateValue {
  type View<'a> = SnapshotStateValue;
}

impl ::protobuf::AsView for SnapshotStateValue {
  type Proxied = SnapshotStateValue;

  fn as_view(&self) -> SnapshotStateValue {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for SnapshotStateValue {
  fn into_view<'shorter>(self) -> SnapshotStateValue where 'msg: 'shorter {
    self
  }
}

// SAFETY: this is an enum type
unsafe impl ::protobuf::__internal::Enum for SnapshotStateValue {
  const NAME: &'static str = "SnapshotStateValue";

  fn is_known(value: i32) -> bool {
    matches!(value, 0|1|2|3|4)
  }
}

impl ::protobuf::__internal::runtime::EntityType for SnapshotStateValue {
    type Tag = ::protobuf::__internal::runtime::EnumTag;
}


