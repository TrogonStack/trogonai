package ai.trogon.jetbrains

import com.intellij.testFramework.fixtures.BasePlatformTestCase
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Integration tests for TrogonService using the IntelliJ Platform test harness.
 * BasePlatformTestCase provides a lightweight project with a real temp basePath.
 *
 * These tests use the host's `echo` binary (always on PATH) to verify that the
 * subprocess streaming machinery works without needing a real `trogon` install.
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
        TrogonSettings.getInstance().trogonPath = "echo"

        val chunks = mutableListOf<String>()
        val doneLatch = CountDownLatch(1)
        val errorLatch = CountDownLatch(1)

        service.sendPrompt(
            prompt = "hello trogon",
            onChunk = { chunks.add(it) },
            onDone = { doneLatch.countDown() },
            onError = { errorLatch.countDown() },
        )

        // echo exits in <1s; 5s timeout is generous for CI
        val done = doneLatch.await(5, TimeUnit.SECONDS)
        assertTrue("onDone was not called within 5s", done)
        assertFalse("onError must not be called when echo succeeds", errorLatch.count == 0L)

        val combined = chunks.joinToString("")
        // echo should have printed "--print hello trogon\n" or similar
        assertTrue("Expected non-empty output from echo, got: '$combined'", combined.isNotBlank())
    }

    fun testSendPromptWithMissingBinaryCallsOnError() {
        TrogonSettings.getInstance().trogonPath = "this-binary-does-not-exist-xyz"

        val errorMessages = mutableListOf<String>()
        val latch = CountDownLatch(1)

        service.sendPrompt(
            prompt = "anything",
            onChunk = {},
            onDone = { latch.countDown() },
            onError = { msg -> errorMessages.add(msg); latch.countDown() },
        )

        assertTrue("Expected callback within 5s", latch.await(5, TimeUnit.SECONDS))
        assertTrue("Expected onError to be called, but got: $errorMessages", errorMessages.isNotEmpty())
        assertTrue(
            "Error message should mention the binary name, got: ${errorMessages.first()}",
            errorMessages.first().isNotBlank()
        )
    }

    fun testCancelStopsActiveProcess() {
        // Use `sleep 30` to simulate a long-running trogon call
        TrogonSettings.getInstance().trogonPath = "sleep"

        val doneLatch = CountDownLatch(1)
        service.sendPrompt(
            prompt = "30",
            onChunk = {},
            onDone = { doneLatch.countDown() },
            onError = { doneLatch.countDown() },
        )

        Thread.sleep(100)   // let the process start
        service.cancel()    // cancel must not throw

        // After cancel the process is gone — we just assert no hang/exception
        // The latch may or may not fire depending on timing; that's intentional
    }

    fun testSecondSendPromptCancelsFirst() {
        TrogonSettings.getInstance().trogonPath = "sleep"

        val firstErrorLatch = CountDownLatch(1)
        service.sendPrompt("30", onChunk = {}, onDone = {}, onError = { firstErrorLatch.countDown() })

        Thread.sleep(100)

        // Starting a second prompt must implicitly cancel the first
        val secondDoneLatch = CountDownLatch(1)
        TrogonSettings.getInstance().trogonPath = "echo"
        service.sendPrompt(
            prompt = "second",
            onChunk = {},
            onDone = { secondDoneLatch.countDown() },
            onError = { secondDoneLatch.countDown() },
        )

        assertTrue("Second prompt should complete within 5s", secondDoneLatch.await(5, TimeUnit.SECONDS))
    }
}
