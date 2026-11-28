import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SettingField, SettingsColumn, SettingsSection } from "@/features/settings/layout";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

/** OpenAI-compatible endpoint (OpenRouter etc.) for API-backed instant chats. */
export function ChatProviderSection() {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: provider } = useQuery({
    queryKey: mq(machine, "chat-provider"),
    queryFn: async () => (await api.GET("/api/chat-provider")).data,
  });
  const [baseUrl, setBaseUrl] = useState<string | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: async () => {
      const res = await api.PUT("/api/chat-provider", {
        body: {
          ...(baseUrl !== null ? { baseUrl } : {}),
          ...(apiKey !== "" ? { apiKey } : {}),
          ...(model !== null ? { model } : {}),
        },
      });
      return res.data;
    },
    onSuccess: (data) => {
      if (data) queryClient.setQueryData(mq(machine, "chat-provider"), data);
      void queryClient.invalidateQueries({ queryKey: mq(machine, "chat-provider-models") });
      setApiKey("");
      setBaseUrl(null);
      setModel(null);
    },
  });

  const dirty = baseUrl !== null || apiKey !== "" || model !== null;

  return (
    <SettingsSection title={t("instant chat · api provider")} className="border-t border-border border-b-0">
      <SettingsColumn className="flex flex-col gap-2">
        <SettingField label={t("base url (openai-compatible)")}>
          <Input
            value={baseUrl ?? provider?.baseUrl ?? ""}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="https://openrouter.ai/api/v1"
            className="font-mono"
          />
        </SettingField>
        <SettingField
          label={t("api key")}
          hint={provider?.hasKey ? t("· saved (enter a new one to replace)") : undefined}
        >
          <Input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={provider?.hasKey ? "••••••••" : "sk-or-…"}
            className="font-mono"
          />
        </SettingField>
        <SettingField label={t("default model")}>
          <Input
            value={model ?? provider?.model ?? ""}
            onChange={(e) => setModel(e.target.value)}
            placeholder="openai/gpt-5.5"
            className="font-mono"
          />
        </SettingField>
        <Button
          variant="secondary"
          className="w-fit"
          disabled={!dirty || save.isPending}
          onClick={() => save.mutate()}
        >
          {t("save")}
        </Button>
      </SettingsColumn>
    </SettingsSection>
  );
}
