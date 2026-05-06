package ai.trogon.jetbrains

import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/**
 * Integration tests for TrogonService using the IntelliJ Platform test harness.
 * BasePlatformTestCase provides a lightweight project with a real temp basePath.
 *
 * These tests use the host's `echo` binary (always on PATH) to verify that the
 * subprocess streaming machinery works without needing a real `trogon` install.
 *
 * Note: tests run on the EDT; use PlatformTestUtil.waitWithEventsDispatching()
 * rather than CountDownLatch.await() to avoid blocking the EDT.
 */
class TrogonServiceTest : BasePlatformTestCase() {

    private lateinit var service: TrogonService

    override fun setUp() {
        super.setUp()
        service = TrogonService(project)
    }

    override fun tearDown() {
        service.cancel()
        super.tearDown()
    }

    fun testCancelWhenIdleDoesNotThrow() {
        // cancel() with no active process must be a no-op
        service.cancel()
        service.cancel()  // double-cancel also safe
    }

    fun testSendPromptWithEchoStreamsOutputAndCallsDone() {
        // Override trogon binary to `echo` — produces output and exits 0
        TrogonSettings.getInstance().trogonPath = "/bin/echo"

        val chunks = mutableListOf<String>()
        val done = AtomicBoolean(false)
        val errorMsg = AtomicReference<String?>(null)

        service.sendPrompt(
            prompt = "hello trogon",
            onChunk = { chunks.add(it) },
            onDone = { done.set(true) },
            onError = { errorMsg.set(it) },
        )

        PlatformTestUtil.waitWithEventsDispatching(
            "onDone was not called within 5s",
            { done.get() || errorMsg.get() != null },
            5,
        )

        assertNull("onError was called: ${errorMsg.get()}", errorMsg.get())
        assertTrue("Expected non-empty output from echo", chunks.any { it.isNotBlank() })
    }

    fun testSendPromptWithMissingBinaryCallsOnError() {
        TrogonSettings.getInstance().trogonPath = "this-binary-does-not-exist-xyz"

        val done = AtomicBoolean(false)
        val errorMsg = AtomicReference<String?>(null)

        service.sendPrompt(
            prompt = "anything",
            onChunk = {},
            onDone = { done.set(true) },
            onError = { errorMsg.set(it) },
        )

        PlatformTestUtil.waitWithEventsDispatching(
            "Expected callback within 5s",
            { done.get() || errorMsg.get() != null },
            5,
        )

        assertFalse("onDone must not be called for missing binary", done.get())
        assertNotNull("onError should be called", errorMsg.get())
        assertTrue("Error message should be non-blank", errorMsg.get()!!.isNotBlank())
    }

    fun testCancelStopsActiveProcess() {
        // Use `sleep 30` to simulate a long-running trogon call
        TrogonSettings.getInstance().trogonPath = "/bin/sleep"

        service.sendPrompt(prompt = "30", onChunk = {}, onDone = {}, onError = {})

        Thread.sleep(100)   // let the process start
        service.cancel()    // cancel must not throw
        // After cancel the process is gone — we just assert no hang/exception
    }

    fun testSecondSendPromptCancelsFirst() {
        TrogonSettings.getInstance().trogonPath = "/bin/sleep"

        service.sendPrompt("30", onChunk = {}, onDone = {}, onError = {})

        Thread.sleep(100)  // let the sleep process start

        // Starting a second prompt must implicitly cancel the first
        val secondDone = AtomicBoolean(false)
        val secondError = AtomicReference<String?>(null)
        TrogonSettings.getInstance().trogonPath = "/bin/echo"
        service.sendPrompt(
            prompt = "second",
            onChunk = {},
            onDone = { secondDone.set(true) },
            onError = { secondError.set(it) },
        )

        PlatformTestUtil.waitWithEventsDispatching(
            "Second prompt should complete within 5s",
            { secondDone.get() || secondError.get() != null },
            5,
        )
        assertTrue(
            "Second prompt should complete (done=${secondDone.get()}, error=${secondError.get()})",
            secondDone.get(),
        )
    }
}
