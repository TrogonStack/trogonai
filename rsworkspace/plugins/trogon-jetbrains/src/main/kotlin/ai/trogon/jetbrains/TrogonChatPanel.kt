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
        val response = slashCommandResponse(cmd)
        if (response == null) {
            // /clear: side-effecting reset
            try { doc.remove(0, doc.length) } catch (_: BadLocationException) {}
            lastResponse.clear()
            applyButton.isVisible = false
            appendStyled("Session cleared.\n\n", promptStyle)
        } else {
            appendStyled(response, if (cmd.startsWith("/help")) promptStyle else errorStyle)
        }
    }

    companion object {
        /**
         * Pure slash-command response — no UI side effects.
         * Returns null for /clear (caller handles the reset), a String for all others.
         * Extracted for unit testing.
         */
        internal fun slashCommandResponse(cmd: String): String? = when (cmd.substringBefore(' ')) {
            "/clear" -> null
            "/help" -> "\nCommands:\n" +
                "  /help    show this help\n" +
                "  /clear   clear the chat\n\n" +
                "Keyboard:\n" +
                "  Enter           send prompt\n" +
                "  Ctrl+Shift+T    ask about selected code (editor action)\n\n"
            else -> "\nUnknown command: $cmd\n\n"
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
        applyButton.isVisible = DiffApplier.hasDiff(lastResponse.toString())
    }

    /**
     * Parse the unified diff in [lastResponse] via DiffApplier and apply changes
     * to project files using IntelliJ's document API (undo-able, shows in local history).
     */
    private fun applyDiff() {
        val basePath = project.basePath ?: return
        val patches = DiffApplier.parse(lastResponse.toString()) { relPath ->
            val vf = LocalFileSystem.getInstance().findFileByPath("$basePath/$relPath")
                ?: return@parse null
            FileDocumentManager.getInstance().getDocument(vf)?.text?.lines()
        }

        if (patches.isEmpty()) {
            appendStyled("\n[no applicable diff found in the last response]\n", promptStyle)
            return
        }

        WriteCommandAction.runWriteCommandAction(project, "Apply Trogon Changes", null, {
            var applied = 0
            patches.forEach { (relPath, newLines) ->
                val vf = LocalFileSystem.getInstance()
                    .findFileByPath("$basePath/$relPath") ?: return@forEach
                val document = FileDocumentManager.getInstance().getDocument(vf)
                    ?: return@forEach
                document.setText(newLines.joinToString("\n"))
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
}
