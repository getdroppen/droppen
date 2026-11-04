import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Figure } from "../figure";
import { NETWORK } from "./model";
import { NetworkLive } from "./live";

/*
 * The connection path as a bench: direct rail below, relay viaduct above, a
 * lift at either end. The copy around it carries the measured times and the
 * two properties the drawing cannot say on its own (switch time, resend order).
 */
export function NetworkFigure() {
  const { t } = useTranslation();
  const direct = `${NETWORK.direct[0]}–${NETWORK.direct[1]} ms`;
  const relay = `${NETWORK.relay[0]}–${NETWORK.relay[1]} ms`;

  const labels = useMemo(
    () => ({
      direct: t("Direct · {{ms}}", { ms: direct }),
      relay: t("Relay · {{ms}}", { ms: relay }),
      switching: t("Switching · 1.5 s"),
    }),
    [t, direct, relay],
  );
  const [readout, setReadout] = useState(labels.direct);
  useEffect(() => setReadout(labels.direct), [labels.direct]);

  return (
    <Figure
      title={t("How it connects")}
      readout={readout}
      keys={[
        { mark: "lit", label: t("Direct · {{ms}}", { ms: direct }) },
        { mark: "dashed", label: t("Relay · {{ms}}", { ms: relay }) },
        { mark: "raised", label: t("Droppen relay") },
        { mark: "dotted", label: t("Encrypted data only") },
      ]}
      caption={
        <>
          {t("Switches within 1.5 s")}. {t("Keys you type are sent again in order")}.
        </>
      }
    >
      <NetworkLive label={t("How it connects")} labels={labels} onReadout={setReadout} />
    </Figure>
  );
}
