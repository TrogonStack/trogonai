pub trait ParseArgs {
    type Args;
    fn parse_args(&self) -> Self::Args;
}

#[cfg(feature = "clap")]
pub struct CliArgs<A: clap::Parser>(std::marker::PhantomData<A>);

#[cfg(feature = "clap")]
impl<A: clap::Parser> CliArgs<A> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[cfg(feature = "clap")]
impl<A: clap::Parser> Default for CliArgs<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "clap")]
impl<A: clap::Parser> ParseArgs for CliArgs<A> {
    type Args = A;
    fn parse_args(&self) -> A {
        A::parse()
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct FixedArgs<A>(pub A);

#[cfg(any(test, feature = "test-support"))]
impl<A: Clone> ParseArgs for FixedArgs<A> {
    type Args = A;
    fn parse_args(&self) -> A {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct TestArgs {
        prefix: String,
    }

    #[test]
    fn fixed_args_returns_provided_value() {
        let args = FixedArgs(TestArgs {
            prefix: "test".to_string(),
        });
        let result = args.parse_args();
        assert_eq!(result.prefix, "test");
    }
}
