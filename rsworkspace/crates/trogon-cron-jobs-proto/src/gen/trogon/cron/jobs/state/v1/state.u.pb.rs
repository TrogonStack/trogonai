const _: () = ::protobuf::__internal::assert_compatible_gencode_version("4.34.1-release");
// This variable must not be referenced except by protobuf generated
// code.
pub(crate) static mut trogon__cron__jobs__state__v1__State_msg_init: ::protobuf::__internal::runtime::MiniTableInitPtr =
    ::protobuf::__internal::runtime::MiniTableInitPtr(::protobuf::__internal::runtime::MiniTablePtr::dangling());
#[allow(non_camel_case_types)]
pub struct State {
  inner: ::protobuf::__internal::runtime::OwnedMessageInner<State>
}

impl ::protobuf::Message for State {}

impl ::std::default::Default for State {
  fn default() -> Self {
    Self::new()
  }
}

impl ::std::fmt::Debug for State {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

// SAFETY:
// - `State` is `Sync` because it does not implement interior mutability.
//    Neither does `StateMut`.
unsafe impl Sync for State {}

// SAFETY:
// - `State` is `Send` because it uniquely owns its arena and does
//   not use thread-local data.
unsafe impl Send for State {}

impl ::protobuf::Proxied for State {
  type View<'msg> = StateView<'msg>;
}

impl ::protobuf::__internal::SealedInternal for State {}

impl ::protobuf::MutProxied for State {
  type Mut<'msg> = StateMut<'msg>;
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct StateView<'msg> {
  inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, State>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for StateView<'msg> {}

impl<'msg> ::protobuf::MessageView<'msg> for StateView<'msg> {
  type Message = State;
}

impl ::std::fmt::Debug for StateView<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl ::std::default::Default for StateView<'_> {
  fn default() -> StateView<'static> {
    ::protobuf::__internal::runtime::MessageViewInner::default().into()
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageViewInner<'msg, State>> for StateView<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageViewInner<'msg, State>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> StateView<'msg> {

  pub fn to_owned(&self) -> State {
    ::protobuf::IntoProxied::into_proxied(*self, ::protobuf::__internal::Private)
  }

  // state: optional enum trogon.cron.jobs.state.v1.StateValue
  pub fn has_state(self) -> bool {
    unsafe {
      self.inner.ptr().has_field_at_index(0)
    }
  }
  pub fn state_opt(self) -> ::protobuf::Optional<super::StateValue> {
        ::protobuf::Optional::new(self.state(), self.has_state())
  }
  pub fn state(self) -> super::StateValue {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::StateValue::Unspecified).into()
      ).try_into().unwrap()
    }
  }

}

// SAFETY:
// - `StateView` is `Sync` because it does not support mutation.
unsafe impl Sync for StateView<'_> {}

// SAFETY:
// - `StateView` is `Send` because while its alive a `StateMut` cannot.
// - `StateView` does not use thread-local data.
unsafe impl Send for StateView<'_> {}

impl<'msg> ::protobuf::AsView for StateView<'msg> {
  type Proxied = State;
  fn as_view(&self) -> ::protobuf::View<'msg, State> {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for StateView<'msg> {
  fn into_view<'shorter>(self) -> StateView<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

impl<'msg> ::protobuf::IntoProxied<State> for StateView<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> State {
    let mut dst = State::new();
    assert!(unsafe {
      dst.inner.ptr_mut().deep_copy(self.inner.ptr(), dst.inner.arena())
    });
    dst
  }
}

impl<'msg> ::protobuf::IntoProxied<State> for StateMut<'msg> {
  fn into_proxied(self, _private: ::protobuf::__internal::Private) -> State {
    ::protobuf::IntoProxied::into_proxied(::protobuf::IntoView::into_view(self), _private)
  }
}

impl ::protobuf::__internal::runtime::EntityType for State {
    type Tag = ::protobuf::__internal::runtime::MessageTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for StateView<'msg> {
    type Tag = ::protobuf::__internal::runtime::ViewProxyTag;
}

impl<'msg> ::protobuf::__internal::runtime::EntityType for StateMut<'msg> {
    type Tag = ::protobuf::__internal::runtime::MutProxyTag;
}

#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct StateMut<'msg> {
  inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, State>,
}

impl<'msg> ::protobuf::__internal::SealedInternal for StateMut<'msg> {}

impl<'msg> ::protobuf::MessageMut<'msg> for StateMut<'msg> {
  type Message = State;
}

impl ::std::fmt::Debug for StateMut<'_> {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    write!(f, "{}", ::protobuf::__internal::runtime::debug_string(self))
  }
}

impl<'msg> From<::protobuf::__internal::runtime::MessageMutInner<'msg, State>> for StateMut<'msg> {
  fn from(inner: ::protobuf::__internal::runtime::MessageMutInner<'msg, State>) -> Self {
    Self { inner }
  }
}

#[allow(dead_code)]
impl<'msg> StateMut<'msg> {

  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private)
    -> ::protobuf::__internal::runtime::MessageMutInner<'msg, State> {
    self.inner
  }

  pub fn to_owned(&self) -> State {
    ::protobuf::AsView::as_view(self).to_owned()
  }

  // state: optional enum trogon.cron.jobs.state.v1.StateValue
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
  pub fn state_opt(&self) -> ::protobuf::Optional<super::StateValue> {
        ::protobuf::Optional::new(self.state(), self.has_state())
  }
  pub fn state(&self) -> super::StateValue {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::StateValue::Unspecified).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_state(&mut self, val: super::StateValue) {
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
// - `StateMut` does not perform any shared mutation.
unsafe impl Send for StateMut<'_> {}

// SAFETY:
// - `StateMut` does not perform any shared mutation.
unsafe impl Sync for StateMut<'_> {}

impl<'msg> ::protobuf::AsView for StateMut<'msg> {
  type Proxied = State;
  fn as_view(&self) -> ::protobuf::View<'_, State> {
    StateView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for StateMut<'msg> {
  fn into_view<'shorter>(self) -> ::protobuf::View<'shorter, State>
  where
      'msg: 'shorter {
    StateView {
      inner: ::protobuf::__internal::runtime::MessageViewInner::view_of_mut(self.inner)
    }
  }
}

impl<'msg> ::protobuf::AsMut for StateMut<'msg> {
  type MutProxied = State;
  fn as_mut(&mut self) -> StateMut<'msg> {
    StateMut { inner: self.inner }
  }
}

impl<'msg> ::protobuf::IntoMut<'msg> for StateMut<'msg> {
  fn into_mut<'shorter>(self) -> StateMut<'shorter>
  where
      'msg: 'shorter {
    self
  }
}

#[allow(dead_code)]
impl State {
  pub fn new() -> Self {
    Self { inner: ::protobuf::__internal::runtime::OwnedMessageInner::<Self>::new() }
  }


  #[doc(hidden)]
  pub fn as_message_mut_inner(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessageMutInner<'_, State> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner)
  }

  pub fn as_view(&self) -> StateView<'_> {
    ::protobuf::__internal::runtime::MessageViewInner::view_of_owned(&self.inner).into()
  }

  pub fn as_mut(&mut self) -> StateMut<'_> {
    ::protobuf::__internal::runtime::MessageMutInner::mut_of_owned(&mut self.inner).into()
  }

  // state: optional enum trogon.cron.jobs.state.v1.StateValue
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
  pub fn state_opt(&self) -> ::protobuf::Optional<super::StateValue> {
        ::protobuf::Optional::new(self.state(), self.has_state())
  }
  pub fn state(&self) -> super::StateValue {
    unsafe {
      // TODO: b/361751487: This .into() and .try_into() is only
      // here for the enum<->i32 case, we should avoid it for
      // other primitives where the types naturally match
      // perfectly (and do an unchecked conversion for
      // i32->enum types, since even for closed enums we trust
      // upb to only return one of the named values).
      self.inner.ptr().get_i32_at_index(
        0, (super::StateValue::Unspecified).into()
      ).try_into().unwrap()
    }
  }
  pub fn set_state(&mut self, val: super::StateValue) {
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

}  // impl State

impl ::std::ops::Drop for State {
  #[inline]
  fn drop(&mut self) {
  }
}

impl ::std::clone::Clone for State {
  fn clone(&self) -> Self {
    self.as_view().to_owned()
  }
}

impl ::protobuf::AsView for State {
  type Proxied = Self;
  fn as_view(&self) -> StateView<'_> {
    self.as_view()
  }
}

impl ::protobuf::AsMut for State {
  type MutProxied = Self;
  fn as_mut(&mut self) -> StateMut<'_> {
    self.as_mut()
  }
}

unsafe impl ::protobuf::__internal::runtime::AssociatedMiniTable for State {
  fn mini_table() -> ::protobuf::__internal::runtime::MiniTablePtr {
    static ONCE_LOCK: ::std::sync::OnceLock<::protobuf::__internal::runtime::MiniTableInitPtr> =
        ::std::sync::OnceLock::new();
    unsafe {
      ONCE_LOCK.get_or_init(|| {
        super::trogon__cron__jobs__state__v1__State_msg_init.0 =
            ::protobuf::__internal::runtime::build_mini_table("$.");
        ::protobuf::__internal::runtime::link_mini_table(
            super::trogon__cron__jobs__state__v1__State_msg_init.0, &[], &[]);
        ::protobuf::__internal::runtime::MiniTableInitPtr(super::trogon__cron__jobs__state__v1__State_msg_init.0)
      }).0
    }
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetArena for State {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for State {
  type Msg = State;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<State> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for State {
  type Msg = State;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<State> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtrMut for StateMut<'_> {
  type Msg = State;
  fn get_ptr_mut(&mut self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<State> {
    self.inner.ptr_mut()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for StateMut<'_> {
  type Msg = State;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<State> {
    self.inner.ptr()
  }
}
unsafe impl ::protobuf::__internal::runtime::UpbGetMessagePtr for StateView<'_> {
  type Msg = State;
  fn get_ptr(&self, _private: ::protobuf::__internal::Private) -> ::protobuf::__internal::runtime::MessagePtr<State> {
    self.inner.ptr()
  }
}

unsafe impl ::protobuf::__internal::runtime::UpbGetArena for StateMut<'_> {
  fn get_arena(&mut self, _private: ::protobuf::__internal::Private) -> &::protobuf::__internal::runtime::Arena {
    self.inner.arena()
  }
}



#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StateValue(i32);

#[allow(non_upper_case_globals)]
impl StateValue {
  pub const Unspecified: StateValue = StateValue(0);
  pub const Missing: StateValue = StateValue(1);
  pub const PresentEnabled: StateValue = StateValue(2);
  pub const PresentDisabled: StateValue = StateValue(3);
  pub const Deleted: StateValue = StateValue(4);

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

impl ::std::convert::From<StateValue> for i32 {
  fn from(val: StateValue) -> i32 {
    val.0
  }
}

impl ::std::convert::From<i32> for StateValue {
  fn from(val: i32) -> StateValue {
    Self(val)
  }
}

impl ::std::default::Default for StateValue {
  fn default() -> Self {
    Self(0)
  }
}

impl ::std::fmt::Debug for StateValue {
  fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
    if let Some(constant_name) = self.constant_name() {
      write!(f, "StateValue::{}", constant_name)
    } else {
      write!(f, "StateValue::from({})", self.0)
    }
  }
}

impl ::protobuf::IntoProxied<i32> for StateValue {
  fn into_proxied(self, _: ::protobuf::__internal::Private) -> i32 {
    self.0
  }
}

impl ::protobuf::__internal::SealedInternal for StateValue {}

impl ::protobuf::Proxied for StateValue {
  type View<'a> = StateValue;
}

impl ::protobuf::AsView for StateValue {
  type Proxied = StateValue;

  fn as_view(&self) -> StateValue {
    *self
  }
}

impl<'msg> ::protobuf::IntoView<'msg> for StateValue {
  fn into_view<'shorter>(self) -> StateValue where 'msg: 'shorter {
    self
  }
}

// SAFETY: this is an enum type
unsafe impl ::protobuf::__internal::Enum for StateValue {
  const NAME: &'static str = "StateValue";

  fn is_known(value: i32) -> bool {
    matches!(value, 0|1|2|3|4)
  }
}

impl ::protobuf::__internal::runtime::EntityType for StateValue {
    type Tag = ::protobuf::__internal::runtime::EnumTag;
}


