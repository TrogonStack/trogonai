package ai.trogon.jetbrains

import org.junit.Assert.*
import org.junit.Test

/**
 * Unit tests for slash command response logic.
 * [TrogonChatPanel.slashCommandResponse] is pure — no IDE or Swing needed.
 */
class SlashCommandTest {

    private fun response(cmd: String) = TrogonChatPanel.slashCommandResponse(cmd)

    @Test
    fun `help returns non-null string listing all commands`() {
        val out = response("/help")
        assertNotNull(out)
        assertTrue("expected /help in response, got: $out", out!!.contains("/help"))
        assertTrue("expected /clear in response, got: $out", out.contains("/clear"))
    }

    @Test
    fun `clear returns null to signal caller-handled reset`() {
        assertNull(response("/clear"))
    }

    @Test
    fun `unknown command returns non-null error string`() {
        val out = response("/nope")
        assertNotNull(out)
        assertTrue("expected 'Unknown' in response, got: $out", out!!.contains("Unknown"))
        assertTrue("expected command echoed, got: $out", out.contains("/nope"))
    }

    @Test
    fun `unknown command with args is still unknown`() {
        val out = response("/compact force")
        assertNotNull(out)
        assertTrue(out!!.contains("Unknown"))
    }

    @Test
    fun `help response includes keyboard shortcut hint`() {
        val out = response("/help")
        assertNotNull(out)
        assertTrue("expected keyboard hint, got: $out", out!!.contains("Ctrl+Shift+T"))
    }
}
