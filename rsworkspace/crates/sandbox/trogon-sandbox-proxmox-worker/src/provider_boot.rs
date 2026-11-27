use trogon_sandbox_contract::LeaseRequest;

use crate::api_bodies::VmConfigBody;
use crate::check::ensure;
use crate::constants::WORKSPACE_SETUP_BUDGET;
use crate::provider_template::nic_keys;
use crate::sizing::cpu_config;
use crate::{LeaseProxy, LeaseRecord, ProxmoxProvider, ProxmoxProviderError, VmPhase, network_setup};
use trogon_sandbox_contract::LeaseId;

impl ProxmoxProvider {
    /// Clones the template into the record's VM, strips and replaces its NICs, sizes it, boots it and waits for
    /// its guest agent.
    pub(crate) async fn boot(
        &self,
        request: &LeaseRequest,
        lease: &LeaseId,
        record: &mut LeaseRecord,
        proxy: Option<&LeaseProxy>,
    ) -> Result<(), ProxmoxProviderError> {
        let upid = self
            .api
            .begin_linked_clone(self.config.template, record.vm, &record.name)
            .await?;
        record.phase = VmPhase::CloneTask { upid: upid.clone() };
        self.save(lease, record).await?;
        self.api.wait_task(&upid).await?;
        record.phase = VmPhase::Ready;
        self.save(lease, record).await?;
        let cloned = self.api.config(record.vm).await?;
        let nics = nic_keys(&cloned)?;
        let cpu = request.spec.vcpu_millis().map(cpu_config);
        let body = VmConfigBody {
            name: &record.name,
            agent: "enabled=1",
            ciuser: "root",
            delete: (!nics.is_empty()).then(|| nics.join(",")),
            memory: request.spec.memory_mib().get(),
            cores: cpu.as_ref().map(|cpu| cpu.cores),
            cpulimit: cpu.as_ref().and_then(|cpu| cpu.cpulimit),
        };
        self.api.update_config(record.vm, &body).await?;
        if let (Some(network), Some(slot), Some(proxy)) = (&self.config.network, record.network_slot, proxy) {
            network_setup::configure(&self.api, record.vm, network, slot, proxy.port).await?;
        }
        self.api.start(record.vm).await?;
        self.api.wait_agent(record.vm, self.config.boot_timeout).await?;
        let command = vec![
            "/bin/mkdir".to_owned(),
            "-p".to_owned(),
            "--".to_owned(),
            record.cwd.clone(),
        ];
        let output = self.api.exec(record.vm, &command, WORKSPACE_SETUP_BUDGET).await?;
        ensure(
            output.exitcode == Some(0),
            ProxmoxProviderError::Backend("workspace setup failed"),
        )
    }
}
