import { z } from "zod/v3";

export const NodeTypesDtoSchema = z.object({
  items: z.array(z.string())
});

export const ConditionNamesDtoSchema = z.object({
  items: z.array(z.string())
});

export const AgentTypesDtoSchema = z.object({
  items: z.array(z.string())
});

export type NodeTypesDto = z.infer<typeof NodeTypesDtoSchema>;
export type ConditionNamesDto = z.infer<typeof ConditionNamesDtoSchema>;
export type AgentTypesDto = z.infer<typeof AgentTypesDtoSchema>;
