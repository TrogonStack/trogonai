package ai.trogon.jetbrains

import org.junit.Assert.*
import org.junit.Test

class TrogonSettingsTest {

    // ── Default state ─────────────────────────────────────────────────────────

    @Test
    fun `default trogonPath is trogon`() {
        assertEquals("trogon", TrogonSettings.State().trogonPath)
    }

    @Test
    fun `default natsUrl is localhost 4222`() {
        assertEquals("nats://localhost:4222", TrogonSettings.State().natsUrl)
    }

    @Test
    fun `default acpPrefix is acp`() {
        assertEquals("acp", TrogonSettings.State().acpPrefix)
    }

    // ── loadState / getState round-trip ───────────────────────────────────────

    @Test
    fun `loadState restores all fields`() {
        val settings = TrogonSettings()
        settings.loadState(
            TrogonSettings.State(
                trogonPath = "/usr/local/bin/trogon",
                natsUrl = "nats://remote:4222",
                acpPrefix = "prod",
            )
        )
        assertEquals("/usr/local/bin/trogon", settings.trogonPath)
        assertEquals("nats://remote:4222", settings.natsUrl)
        assertEquals("prod", settings.acpPrefix)
    }

    @Test
    fun `getState reflects loadState`() {
        val settings = TrogonSettings()
        val s = TrogonSettings.State(trogonPath = "custom")
        settings.loadState(s)
        assertEquals("custom", settings.getState().trogonPath)
    }

    // ── Property setters ──────────────────────────────────────────────────────

    @Test
    fun `setting trogonPath updates internal state`() {
        val settings = TrogonSettings()
        settings.trogonPath = "/opt/trogon"
        assertEquals("/opt/trogon", settings.state.trogonPath)
    }

    @Test
    fun `setting natsUrl updates internal state`() {
        val settings = TrogonSettings()
        settings.natsUrl = "nats://custom:5555"
        assertEquals("nats://custom:5555", settings.state.natsUrl)
    }

    @Test
    fun `setting acpPrefix updates internal state`() {
        val settings = TrogonSettings()
        settings.acpPrefix = "dev"
        assertEquals("dev", settings.state.acpPrefix)
    }

    @Test
    fun `multiple sets overwrite previous value`() {
        val settings = TrogonSettings()
        settings.trogonPath = "first"
        settings.trogonPath = "second"
        assertEquals("second", settings.trogonPath)
    }
}
