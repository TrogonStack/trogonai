package ai.trogon.jetbrains

import com.intellij.openapi.options.Configurable
import com.intellij.openapi.ui.DialogPanel
import com.intellij.ui.dsl.builder.bindText
import com.intellij.ui.dsl.builder.panel
import javax.swing.JComponent

class TrogonSettingsConfigurable : Configurable {

    private lateinit var panel: DialogPanel

    override fun getDisplayName() = "Trogon AI"

    override fun createComponent(): JComponent {
        val settings = TrogonSettings.getInstance()
        panel = panel {
            row("Trogon binary path:") {
                textField().bindText(settings::trogonPath)
                    .comment("Name or absolute path to the <code>trogon</code> CLI (must be on PATH, or specify full path).")
            }
            row("NATS URL:") {
                textField().bindText(settings::natsUrl)
                    .comment("Default: <code>nats://localhost:4222</code>")
            }
            row("ACP prefix:") {
                textField().bindText(settings::acpPrefix)
                    .comment("Default: <code>acp</code>")
            }
        }
        return panel
    }

    override fun isModified() = panel.isModified()

    override fun apply() = panel.apply()

    override fun reset() = panel.reset()
}
