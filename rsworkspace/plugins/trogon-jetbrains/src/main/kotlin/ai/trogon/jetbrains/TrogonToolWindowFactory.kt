package ai.trogon.jetbrains

import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory

class TrogonToolWindowFactory : ToolWindowFactory {

    override fun createToolWindowContent(project: Project, toolWindow: ToolWindow) {
        val panel = TrogonChatPanel(project)
        val content = toolWindow.contentManager.factory
            .createContent(panel, /* displayName = */ null, /* isLockable = */ false)
        toolWindow.contentManager.addContent(content)
    }

    /** Show the tool window on all projects — no shouldBeAvailable restriction. */
    override fun shouldBeAvailable(project: Project) = true
}
