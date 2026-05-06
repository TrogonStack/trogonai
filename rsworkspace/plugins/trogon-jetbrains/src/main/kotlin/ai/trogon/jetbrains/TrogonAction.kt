package ai.trogon.jetbrains

import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.wm.ToolWindowManager

/**
 * Editor action: right-click → "Ask Trogon" (also Ctrl+Shift+T).
 *
 * Gets the selected text and file name from the editor, opens (or focuses)
 * the Trogon tool window, and sends the selection as a prompt.
 */
class TrogonAction : AnAction() {

    override fun getActionUpdateThread() = ActionUpdateThread.BGT

    override fun update(e: AnActionEvent) {
        val editor = e.getData(CommonDataKeys.EDITOR)
        val hasSelection = editor?.selectionModel?.hasSelection() == true
        e.presentation.isEnabledAndVisible = hasSelection
    }

    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        val editor = e.getData(CommonDataKeys.EDITOR) ?: return
        val selectedText = editor.selectionModel.selectedText?.trim() ?: return
        if (selectedText.isEmpty()) return

        val fileName = e.getData(CommonDataKeys.VIRTUAL_FILE)?.name ?: "unknown"

        // Open (or focus) the Trogon tool window.
        val toolWindow = ToolWindowManager.getInstance(project).getToolWindow("Trogon") ?: return
        toolWindow.show {
            // After the tool window is shown, route the selection to the chat panel.
            val content = toolWindow.contentManager.selectedContent ?: return@show
            val panel = content.component as? TrogonChatPanel ?: return@show
            panel.askAboutSelection(selectedText, fileName)
        }
    }
}
