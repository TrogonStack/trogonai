use super::ActorResourcesError;
use super::constants::{BYTES_PER_MIB, MAX_CPU_MILLICORES};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActorResources {
    cpu_millicores: u32,
    memory_mib: u32,
}

impl ActorResources {
    pub fn new(cpu_millicores: u32, memory_mib: u32) -> Result<Self, ActorResourcesError> {
        if !(1..=MAX_CPU_MILLICORES).contains(&cpu_millicores) || memory_mib == 0 {
            return Err(ActorResourcesError);
        }
        Ok(Self {
            cpu_millicores,
            memory_mib,
        })
    }

    pub fn cpu_millicores(self) -> u32 {
        self.cpu_millicores
    }

    pub fn memory_mib(self) -> u32 {
        self.memory_mib
    }

    pub fn as_proto(self) -> crate::proto::ResourceRequirements {
        crate::proto::ResourceRequirements {
            limits: Some(crate::proto::Resources {
                cpu_millicores: i64::from(self.cpu_millicores),
                memory_bytes: i64::from(self.memory_mib) * BYTES_PER_MIB,
            }),
        }
    }
}
