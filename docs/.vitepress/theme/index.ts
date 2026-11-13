import type { Theme } from "vitepress";
import DefaultTheme from "vitepress/theme";
import AgentGateway from "./AgentGateway.vue";
import AgentGuardrails from "./AgentGuardrails.vue";
import AgentReach from "./AgentReach.vue";
import AgentRevisions from "./AgentRevisions.vue";
import AgentWorkflow from "./AgentWorkflow.vue";
import BuildProgress from "./BuildProgress.vue";
import ConnectorCapabilities from "./ConnectorCapabilities.vue";
import ConnectorGallery from "./ConnectorGallery.vue";
import ConnectorHeader from "./ConnectorHeader.vue";
import ConnectorIngress from "./ConnectorIngress.vue";
import ConnectorVerification from "./ConnectorVerification.vue";
import FeatureGrid from "./FeatureGrid.vue";
import KeyCustodyTiers from "./KeyCustodyTiers.vue";
import KeyEnvelope from "./KeyEnvelope.vue";
import KeyLifecycle from "./KeyLifecycle.vue";
import ProductHeader from "./ProductHeader.vue";
import ProductOverview from "./ProductOverview.vue";
import SecretFlow from "./SecretFlow.vue";
import SessionEventDriven from "./SessionEventDriven.vue";
import SessionLineage from "./SessionLineage.vue";
import SessionPipeline from "./SessionPipeline.vue";
import SessionRecovery from "./SessionRecovery.vue";
import SessionTape from "./SessionTape.vue";
import "./custom.css";

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component("AgentGateway", AgentGateway);
    app.component("AgentGuardrails", AgentGuardrails);
    app.component("AgentReach", AgentReach);
    app.component("AgentRevisions", AgentRevisions);
    app.component("AgentWorkflow", AgentWorkflow);
    app.component("BuildProgress", BuildProgress);
    app.component("ConnectorCapabilities", ConnectorCapabilities);
    app.component("ConnectorGallery", ConnectorGallery);
    app.component("ConnectorHeader", ConnectorHeader);
    app.component("ConnectorIngress", ConnectorIngress);
    app.component("ConnectorVerification", ConnectorVerification);
    app.component("FeatureGrid", FeatureGrid);
    app.component("KeyCustodyTiers", KeyCustodyTiers);
    app.component("KeyEnvelope", KeyEnvelope);
    app.component("KeyLifecycle", KeyLifecycle);
    app.component("ProductHeader", ProductHeader);
    app.component("ProductOverview", ProductOverview);
    app.component("SecretFlow", SecretFlow);
    app.component("SessionEventDriven", SessionEventDriven);
    app.component("SessionLineage", SessionLineage);
    app.component("SessionPipeline", SessionPipeline);
    app.component("SessionRecovery", SessionRecovery);
    app.component("SessionTape", SessionTape);
  },
} satisfies Theme;
