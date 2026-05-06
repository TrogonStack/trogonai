package ai.trogon.jetbrains

import com.intellij.openapi.components.*

@Service(Service.Level.APP)
@State(
    name = "TrogonSettings",
    storages = [Storage("TrogonSettings.xml")]
)
class TrogonSettings : PersistentStateComponent<TrogonSettings.State> {

    data class State(
        var trogonPath: String = "trogon",
        var natsUrl: String = "nats://localhost:4222",
        var acpPrefix: String = "acp",
    )

    internal var state = State()

    override fun getState(): State = state

    override fun loadState(s: State) {
        state = s
    }

    var trogonPath: String
        get() = state.trogonPath
        set(v) { state.trogonPath = v }

    var natsUrl: String
        get() = state.natsUrl
        set(v) { state.natsUrl = v }

    var acpPrefix: String
        get() = state.acpPrefix
        set(v) { state.acpPrefix = v }

    companion object {
        fun getInstance(): TrogonSettings = service()
    }
}
