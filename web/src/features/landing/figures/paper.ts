/** The drafting grid the public pages draw their figures on. */
export const gridPaper = (grid = 22) => ({
  backgroundImage: "linear-gradient(var(--color-border) 1px, transparent 1px), linear-gradient(90deg, var(--color-border) 1px, transparent 1px)",
  backgroundSize: `${grid}px ${grid}px`,
  backgroundPosition: "-1px -1px",
});
