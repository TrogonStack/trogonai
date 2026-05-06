package ai.trogon.jetbrains

import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.ui.components.JBScrollPane
import com.intellij.util.ui.JBUI
import java.awt.BorderLayout
import java.awt.Color
import java.awt.FlowLayout
import javax.swing.*
import javax.swing.text.*

/**
 * Main chat panel shown in the Trogon tool window.
 *
 * Layout:
 *   ┌──────────────────────────────┐
 *   │  output (JTextPane, styled)  │
 *   ├──────────────────────────────┤
 *   │  [Apply Changes] (hidden)    │
 *   │  input field  [Cancel][Send] │
 *   └──────────────────────────────┘
 *
 * Diff display: lines prefixed with `+` are green, `-` are red, `@@` are blue.
 * Apply Changes: saves the diff to a temp file and invokes IntelliJ's
 * ApplyPatchAction so the user gets the standard accept/reject diff viewer.
 */
class TrogonChatPanel(private val project: Project) : JPanel(BorderLayout()) {

    private val service: TrogonService = project.getService(TrogonService::class.java)

    // ── Output ────────────────────────────────────────────────────────────────

    private val outputPane = JTextPane().apply {
        isEditable = false
        border = JBUI.Borders.empty(4)
    }
    private val doc: StyledDocument = outputPane.styledDocument

    // Styles
    private val normalStyle = doc.addStyle("normal", null)
    private val promptStyle = doc.addStyle("prompt", null).also {
        StyleConstants.setForeground(it, Color(130, 130, 130))
        StyleConstants.setItalic(it, true)
    }
    private val addStyle = doc.addStyle("add", null).also {
        StyleConstants.setForeground(it, Color(80, 200, 80))
    }
    private val removeStyle = doc.addStyle("remove", null).also {
        StyleConstants.setForeground(it, Color(220, 80, 80))
    }
    private val hunkStyle = doc.addStyle("hunk", null).also {
        StyleConstants.setForeground(it, Color(80, 160, 220))
        StyleConstants.setBold(it, true)
    }
    private val headerStyle = doc.addStyle("header", null).also {
        StyleConstants.setBold(it, true)
    }
    private val errorStyle = doc.addStyle("error", null).also {
        StyleConstants.setForeground(it, Color(220, 80, 80))
        StyleConstants.setBold(it, true)
    }

    // ── Controls ──────────────────────────────────────────────────────────────

    private val inputField = JTextField()
    private val sendButton = JButton("Send")
    private val cancelButton = JButton("Cancel").apply { isEnabled = false }
    private val applyButton = JButton("Apply Changes").apply {
        isVisible = false
        toolTipText = "Apply the diff in the last response to the affected files"
    }

    // Accumulated text of the last response, for diff detection and apply.
    private val lastResponse = StringBuilder()

    init {
        buildUi()
    }

    private fun buildUi() {
        val inputPanel = JPanel(BorderLayout(4, 0)).apply {
            add(inputField, BorderLayout.CENTER)
            add(JPanel(FlowLayout(FlowLayout.RIGHT, 4, 0)).apply {
                add(cancelButton)
                add(sendButton)
            }, BorderLayout.EAST)
        }

        val bottomPanel = JPanel(BorderLayout(0, 4)).apply {
            border = JBUI.Borders.emptyTop(4)
            add(applyButton, BorderLayout.NORTH)
            add(inputPanel, BorderLayout.SOUTH)
        }

        add(JBScrollPane(outputPane), BorderLayout.CENTER)
        add(bottomPanel, BorderLayout.SOUTH)
        border = JBUI.Borders.empty(8)

        sendButton.addActionListener { sendPrompt() }
        cancelButton.addActionListener { cancel() }
        applyButton.addActionListener { applyDiff() }
        inputField.addActionListener { sendPrompt() }

        appendStyled(
            "Trogon AI — type a message or use /help\n\n",
            promptStyle
        )
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /** Pre-fill and send a prompt (called from TrogonAction with selected text). */
    fun askAboutSelection(selectedText: String, fileName: String) {
        val prompt = "Explain or help with the following code from `$fileName`:\n\n```\n$selectedText\n```"
        submit(prompt, displayLabel = "[selection from $fileName]")
    }

    // ── Prompt handling ───────────────────────────────────────────────────────

    private fun sendPrompt() {
        val raw = inputField.text.trim()
        if (raw.isEmpty()) return
        inputField.text = ""

        if (raw.startsWith("/")) {
            handleSlashCommand(raw)
            return
        }

        submit(raw, displayLabel = raw)
    }

    private fun submit(prompt: String, displayLabel: String) {
        lastResponse.clear()
        applyButton.isVisible = false
        sendButton.isEnabled = false
        cancelButton.isEnabled = true

        appendStyled("\n> $displayLabel\n", promptStyle)

        service.sendPrompt(
            prompt,
            onChunk = { chunk ->
                lastResponse.append(chunk)
                appendChunk(chunk)
            },
            onDone = {
                appendStyled("\n", normalStyle)
                sendButton.isEnabled = true
                cancelButton.isEnabled = false
                checkForDiff()
            },
            onError = { err ->
                appendStyled("\n[error: $err]\n", errorStyle)
                sendButton.isEnabled = true
                cancelButton.isEnabled = false
            }
        )
    }

    private fun cancel() {
        service.cancel()
        sendButton.isEnabled = true
        cancelButton.isEnabled = false
        appendStyled("\n[cancelled]\n", promptStyle)
    }

    // ── Slash commands ────────────────────────────────────────────────────────

    private fun handleSlashCommand(cmd: String) {
        when (cmd.substringBefore(' ')) {
            "/clear" -> {
                try { doc.remove(0, doc.length) } catch (_: BadLocationException) {}
                lastResponse.clear()
                applyButton.isVisible = false
                appendStyled("Session cleared.\n\n", promptStyle)
            }
            "/help" -> appendStyled(
                "\nCommands:\n" +
                "  /help    show this help\n" +
                "  /clear   clear the chat\n\n" +
                "Keyboard:\n" +
                "  Enter           send prompt\n" +
                "  Ctrl+Shift+T    ask about selected code (editor action)\n\n",
                promptStyle
            )
            else -> appendStyled("\nUnknown command: $cmd\n\n", errorStyle)
        }
    }

    // ── Styled output ─────────────────────────────────────────────────────────

    /**
     * Append a streaming chunk to the output pane.
     * Lines starting with diff markers get coloured; everything else is normal.
     * Handles partial lines correctly: colour is determined by line start only.
     */
    private fun appendChunk(chunk: String) {
        // Split into runs separated by newlines, preserving structure.
        val parts = chunk.split('\n')
        parts.forEachIndexed { i, line ->
            if (i > 0) appendStyled("\n", normalStyle)
            if (line.isEmpty()) return@forEachIndexed
            val style = when {
                line.startsWith("+++ ") || line.startsWith("--- ") -> headerStyle
                line.startsWith("@@") -> hunkStyle
                line.startsWith("+") && !line.startsWith("+++") -> addStyle
                line.startsWith("-") && !line.startsWith("---") -> removeStyle
                else -> normalStyle
            }
            appendStyled(line, style)
        }
    }

    private fun appendStyled(text: String, style: Style) {
        try {
            doc.insertString(doc.length, text, style)
            outputPane.caretPosition = doc.length
        } catch (_: BadLocationException) {}
    }

    // ── Diff apply ────────────────────────────────────────────────────────────

    private fun checkForDiff() {
        val resp = lastResponse.toString()
        val hasDiff = resp.lines().any { it.startsWith("+++ ") || it.startsWith("--- ") }
        applyButton.isVisible = hasDiff
    }

    /**
     * Parse the unified diff in [lastResponse] and apply it to the project files
     * using IntelliJ's document API inside a write command action.
     * Each file modified shows up in the local history and can be undone.
     */
    private fun applyDiff() {
        val patches = parseDiff(lastResponse.toString(), project.basePath ?: return)
        if (patches.isEmpty()) {
            appendStyled("\n[no applicable diff found in the last response]\n", promptStyle)
            return
        }

        WriteCommandAction.runWriteCommandAction(project, "Apply Trogon Changes", null, {
            var applied = 0
            patches.forEach { (path, newContent) ->
                val vf = LocalFileSystem.getInstance().findFileByPath(path) ?: return@forEach
                val document = FileDocumentManager.getInstance().getDocument(vf) ?: return@forEach
                document.setText(newContent)
                applied++
            }
            onEdt {
                if (applied > 0) {
                    appendStyled("\n[applied changes to $applied file(s)]\n", addStyle)
                    applyButton.isVisible = false
                } else {
                    appendStyled("\n[could not locate the target files in this project]\n", promptStyle)
                }
            }
        })
    }

    private fun onEdt(block: () -> Unit) =
        com.intellij.openapi.application.ApplicationManager.getApplication().invokeLater(block)

    // ── Diff parser ───────────────────────────────────────────────────────────

    /**
     * Parse a unified diff and produce a map of absolute file path → new content.
     * Reads the current file content and applies hunks line by line.
     * Returns an empty map if parsing fails or the files are not found.
     */
    private fun parseDiff(diff: String, basePath: String): Map<String, String> {
        val result = mutableMapOf<String, String>()
        val lines = diff.lines()
        var i = 0

        while (i < lines.size) {
            // Find next file header
            if (!lines[i].startsWith("--- ")) { i++; continue }
            val minusLine = lines[i]
            if (i + 1 >= lines.size || !lines[i + 1].startsWith("+++ ")) { i++; continue }
            val plusLine = lines[i + 1]
            i += 2

            // Extract relative path (strip "a/" / "b/" prefixes from git diff)
            val relPath = plusLine.removePrefix("+++ ")
                .removePrefix("b/")
                .removePrefix("/")
                .trim()
            val absPath = "$basePath/$relPath"

            val vf = LocalFileSystem.getInstance().findFileByPath(absPath) ?: continue
            val document = FileDocumentManager.getInstance().getDocument(vf) ?: continue
            val originalLines = document.text.split('\n').toMutableList()

            // Collect and apply hunks
            val modified = applyHunks(originalLines, lines, i).also { (_, nextI) -> i = nextI }

            result[absPath] = modified.first.joinToString("\n")
        }

        return result
    }

    /**
     * Apply all hunks starting at [startIdx] in [diffLines] against [original].
     * Returns the modified lines and the index in [diffLines] where we stopped.
     */
    private fun applyHunks(
        original: MutableList<String>,
        diffLines: List<String>,
        startIdx: Int,
    ): Pair<List<String>, Int> {
        val out = original.toMutableList()
        var i = startIdx
        var offset = 0   // cumulative line offset from previous hunks

        while (i < diffLines.size) {
            val line = diffLines[i]
            // Stop at next file header or end
            if (line.startsWith("--- ") && i + 1 < diffLines.size && diffLines[i + 1].startsWith("+++ ")) break
            if (!line.startsWith("@@")) { i++; continue }

            // Parse @@ -startLine,count +startLine,count @@
            val hunkHeader = Regex("""^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@""").find(line)
                ?: run { i++; continue }
            val oldStart = hunkHeader.groupValues[1].toInt() - 1  // 0-based
            i++

            var pos = oldStart + offset
            while (i < diffLines.size) {
                val hunkLine = diffLines[i]
                when {
                    hunkLine.startsWith("@@") -> break
                    hunkLine.startsWith("--- ") -> break
                    hunkLine.startsWith("+") -> {
                        // Insert: add line at current position
                        val content = hunkLine.substring(1)
                        if (pos <= out.size) out.add(pos, content) else out.add(content)
                        pos++
                        offset++
                    }
                    hunkLine.startsWith("-") -> {
                        // Delete: remove line at current position
                        if (pos < out.size) { out.removeAt(pos); offset-- }
                    }
                    hunkLine.startsWith(" ") -> {
                        // Context: advance
                        pos++
                    }
                    // "\ No newline at end of file" and similar — skip
                }
                i++
            }
        }

        return Pair(out, i)
    }
}
