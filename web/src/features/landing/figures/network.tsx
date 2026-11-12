import { useTranslation } from "react-i18next";
import { Computer, DEVICE_LABEL, DiagramText, Ground, INK, LABEL_AT, MODEL_STYLE, MovingPacket, Phone, PORTS, ServerRack, VIEW_BOX, Figure } from "./kit";

/** Measured round trips, in milliseconds. */
const NETWORK = { direct: [63, 73] as const, relay: [215, 227] as const };

interface NetLabels {
  direct: string;
  relay: string;
  relayName: string;
  computer: string;
  phone: string;
}

function NetworkLive({ label, labels }: { label: string; labels: NetLabels }) {
  return (
    <svg viewBox={VIEW_BOX} role="img" aria-label={label} focusable="false" className="block h-auto w-full" fill="none" style={MODEL_STYLE}>
      <Ground />
      {/* The direct path goes over the relay, not into it: a dashed line that meets
          the relay's left port would read as a hop through the relay. */}
      <path d={`M${PORTS.pc}C${PORTS.pc.split(" ")[0]} 120 220 72 300 72H640C700 72 735 110 ${PORTS.client}`} stroke={INK.muted} strokeWidth={1.1} strokeDasharray="4 4" vectorEffect="non-scaling-stroke" />
      <DiagramText x={450} y={62} className="max-lg:hidden">{labels.direct}</DiagramText>
      <path d={`M${PORTS.pc}C250 193 285 220 350 220C395 220 390 156 ${PORTS.relayLeft}M${PORTS.relayRight}C510 162 545 205 610 205C670 205 690 160 ${PORTS.client}`} stroke={INK.fg} strokeWidth={1.15} vectorEffect="non-scaling-stroke" />
      <DiagramText x={650} y={232} className="max-lg:hidden">{labels.relay}</DiagramText>
      <MovingPacket path={`M${PORTS.pc}C250 193 285 220 350 220C395 220 390 156 ${PORTS.relayLeft}L${PORTS.relayRight}C510 162 545 205 610 205C670 205 690 160 ${PORTS.client}`} rest={{ x: 320, y: 220 }} duration={4.4} />
      <Computer />
      <Phone />
      <ServerRack />
      <DiagramText x={LABEL_AT.pc[0]} y={LABEL_AT.pc[1]} ink="fg" size={11} className={DEVICE_LABEL}>{labels.computer}</DiagramText>
      <DiagramText x={LABEL_AT.client[0]} y={LABEL_AT.client[1]} ink="fg" size={11} className={DEVICE_LABEL}>{labels.phone}</DiagramText>
      <DiagramText x={LABEL_AT.relay[0]} y={LABEL_AT.relay[1]} ink="fg" size={11} className={DEVICE_LABEL}>{labels.relayName}</DiagramText>
    </svg>
  );
}

export function NetworkFigure() {
  const { t } = useTranslation();
  const title = t("How it connects");
  const labels: NetLabels = {
    direct: t("Direct · {{ms}}", { ms: `${NETWORK.direct[0]}–${NETWORK.direct[1]} ms` }),
    relay: t("Relay · {{ms}}", { ms: `${NETWORK.relay[0]}–${NETWORK.relay[1]} ms` }),
    relayName: t("Droppen relay"),
    computer: t("Computer"),
    phone: t("Phone"),
  };

  return (
    <Figure>
      <NetworkLive label={title} labels={labels} />
    </Figure>
  );
}
