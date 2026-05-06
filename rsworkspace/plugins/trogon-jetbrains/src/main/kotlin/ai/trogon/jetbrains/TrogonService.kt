package ai.trogon.jetbrains

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.Service
import com.intellij.openapi.project.Project
import java.io.File

/**
 * Project-level service that owns the trogon-cli subprocess.
 *
 * One process per prompt; starting a new prompt cancels the previous one.
 * Communication: `trogon --print "<prompt>"` via stdout streaming (text mode).
 * NATS_URL and ACP_PREFIX are forwarded via env so the CLI connects to the
 * right server without extra flags.
 */
@Service(Service.Level.PROJECT)
class TrogonService(private val project: Project) {

    @Volatile
    private var currentProcess: Process? = null

    /** Cancel any in-flight prompt. */
    fun cancel() {
        currentProcess?.destroyForcibly()
        currentProcess = null
    }

    /**
     * Send [prompt] to trogon-cli and stream the response.
     *
     * All callbacks are invoked on the EDT.
     *
     * @param onChunk  called for each text chunk arriving from stdout
     * @param onDone   called when the process exits with code 0
     * @param onError  called when the process exits non-zero or cannot start
     */
    fun sendPrompt(
        prompt: String,
        onChunk: (String) -> Unit,
        onDone: () -> Unit,
        onError: (String) -> Unit,
    ) {
        cancel()

        val settings = TrogonSettings.getInstance()
        val cwd = File(project.basePath ?: System.getProperty("user.home"))

        val process = try {
            ProcessBuilder(settings.trogonPath, "--print", prompt)
                .directory(cwd)
                .redirectErrorStream(false)
                .also { pb ->
                    pb.environment()["TROGON_NATS_URL"] = settings.natsUrl
                    pb.environment()["ACP_PREFIX"] = settings.acpPrefix
                }
                .start()
        } catch (e: Exception) {
            onEdt { onError("Cannot start trogon: ${e.message}") }
            return
        }

        currentProcess = process

        Thread({
            try {
                process.inputStream.bufferedReader().use { reader ->
                    val buf = CharArray(512)
                    var n: Int
                    while (reader.read(buf).also { n = it } != -1) {
                        val chunk = String(buf, 0, n)
                        onEdt { onChunk(chunk) }
                    }
                }
                val exit = process.waitFor()
                onEdt {
                    if (exit == 0) onDone()
                    else onError("trogon exited with code $exit")
                }
            } catch (_: InterruptedException) {
                // cancelled — no callback
            } catch (e: Exception) {
                onEdt { onError(e.message ?: "unknown error") }
            } finally {
                currentProcess = null
            }
        }, "trogon-reader").apply { isDaemon = true }.start()
    }

    private fun onEdt(block: () -> Unit) =
        ApplicationManager.getApplication().invokeLater(block)
}
