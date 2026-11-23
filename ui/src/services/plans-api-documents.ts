// Plans tab (initiative 8ad3ae71, phase 01): the GraphQL operation documents for draft plans and
// their versioned brief. Pure strings — no Angular — so `web/src/app/pages/plans/plans-api-contract.spec.ts`
// can import them and walk every selection against `worker/schema/johnnyone-ai.graphql`.
//
// `JohnnyApiService` is the only caller. Operations returning `AgentPlanRun` select
// `PLAN_RUN_FIELDS_PLACEHOLDER`, which the service replaces with its shared `agentPlanRunFields`
// (one selection for every AgentPlanRun in the app, not a fork).

export const PLAN_RUN_FIELDS_PLACEHOLDER = '__AGENT_PLAN_RUN_FIELDS__';

const BRIEF_FIELDS = `outcome why where doneWhen outOfScope decided`;
const OUTLINE_TASK = `id title author addedIn`;
const OUTLINE = `phases { id title author addedIn nextTask tasks { ${OUTLINE_TASK} } } nextPhase`;
const WRITE_RESULT = `version changedFields id`;

export const PLANS_DOCUMENTS = {
  listDraftPlans: `query ListDraftPlans {
    listDraftPlans {
      id title workspacePath updatedAt
      latest { version author createdAt }
      checkSummary { ok warn fail }
      queuedAfter
    }
  }`,

  getBriefVersion: `query GetBriefVersion($initiativeId: ID!, $version: Int) {
    getBriefVersion(initiativeId: $initiativeId, version: $version) {
      initiativeId version author changeNote changed createdAt
      fields { ${BRIEF_FIELDS} }
      outline { ${OUTLINE} }
      initiativeStatus acceptedBriefVersion
    }
  }`,

  listBriefVersions: `query ListBriefVersions($initiativeId: ID!) {
    listBriefVersions(initiativeId: $initiativeId) { version author changeNote changed createdAt }
  }`,

  getBriefDiff: `query GetBriefDiff($initiativeId: ID!, $fromVersion: Int!, $toVersion: Int!) {
    getBriefDiff(initiativeId: $initiativeId, fromVersion: $fromVersion, toVersion: $toVersion) {
      fields { field changed removed added }
      outline {
        added { id title }
        removed { id title }
        renamed { id before after }
        moved
      }
    }
  }`,

  getBriefCheck: `query GetBriefCheck($initiativeId: ID!) {
    getBriefCheck(initiativeId: $initiativeId) { rule status detail paths initiativeIds }
  }`,

  createDraftPlan: `mutation CreateDraftPlan($input: CreateDraftPlanInput!) {
    createDraftPlan(input: $input) { ${PLAN_RUN_FIELDS_PLACEHOLDER} }
  }`,

  createDraftPlanDuplicate: `mutation CreateDraftPlanDuplicate($initiativeId: ID!) {
    createDraftPlanDuplicate(initiativeId: $initiativeId) { ${PLAN_RUN_FIELDS_PLACEHOLDER} }
  }`,

  updateBriefField: `mutation UpdateBriefField($initiativeId: ID!, $field: String!, $op: String!, $value: String, $values: [String!]) {
    updateBriefField(initiativeId: $initiativeId, field: $field, op: $op, value: $value, values: $values) { ${WRITE_RESULT} }
  }`,

  resetBriefToVersion: `mutation ResetBriefToVersion($initiativeId: ID!, $version: Int!, $field: String) {
    resetBriefToVersion(initiativeId: $initiativeId, version: $version, field: $field) { ${WRITE_RESULT} }
  }`,

  addOutlinePhase: `mutation AddOutlinePhase($initiativeId: ID!, $title: String!) {
    addOutlinePhase(initiativeId: $initiativeId, title: $title) { ${WRITE_RESULT} }
  }`,

  addOutlineTask: `mutation AddOutlineTask($initiativeId: ID!, $phaseId: String!, $title: String!) {
    addOutlineTask(initiativeId: $initiativeId, phaseId: $phaseId, title: $title) { ${WRITE_RESULT} }
  }`,

  updateOutlineItem: `mutation UpdateOutlineItem($initiativeId: ID!, $id: String!, $title: String!) {
    updateOutlineItem(initiativeId: $initiativeId, id: $id, title: $title) { ${WRITE_RESULT} }
  }`,

  updateOutlineItemPosition: `mutation UpdateOutlineItemPosition($initiativeId: ID!, $id: String!, $toIndex: Int!) {
    updateOutlineItemPosition(initiativeId: $initiativeId, id: $id, toIndex: $toIndex) { ${WRITE_RESULT} }
  }`,

  deleteOutlineItem: `mutation DeleteOutlineItem($initiativeId: ID!, $id: String!) {
    deleteOutlineItem(initiativeId: $initiativeId, id: $id) { ${WRITE_RESULT} }
  }`,

  updatePlanningQueue: `mutation UpdatePlanningQueue($initiativeId: ID!, $afterInitiativeIds: [ID!]) {
    updatePlanningQueue(initiativeId: $initiativeId, afterInitiativeIds: $afterInitiativeIds) { ${PLAN_RUN_FIELDS_PLACEHOLDER} }
  }`,
} as const;

export type PlansOperation = keyof typeof PLANS_DOCUMENTS;

/** Resolve a document, substituting the caller's shared AgentPlanRun selection. */
export function plansDocument(op: PlansOperation, agentPlanRunFields: string): string {
  return PLANS_DOCUMENTS[op].split(PLAN_RUN_FIELDS_PLACEHOLDER).join(agentPlanRunFields);
}
