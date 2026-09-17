import {
  createContext,
  useContext,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
  type TableHTMLAttributes,
} from "react";

export const DESIGN_SYSTEM_VERSION = 1 as const;

type DesignSystemContract = {
  version: typeof DESIGN_SYSTEM_VERSION;
  theme: "dark";
};

const DesignSystemContext = createContext<DesignSystemContract>({
  version: DESIGN_SYSTEM_VERSION,
  theme: "dark",
});

function classNames(...values: Array<string | undefined>) {
  return values.filter(Boolean).join(" ");
}

export function DesignSystemProvider({ children }: { children: ReactNode }) {
  return (
    <DesignSystemContext.Provider value={{ version: DESIGN_SYSTEM_VERSION, theme: "dark" }}>
      <div
        className="ds-root"
        data-design-system-version={DESIGN_SYSTEM_VERSION}
        data-saaa-theme="dark"
      >
        {children}
      </div>
    </DesignSystemContext.Provider>
  );
}

export function useDesignSystem() {
  return useContext(DesignSystemContext);
}

export function Grid({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div {...props} className={classNames("ds-grid", className)} data-ds-component="grid" />;
}

export function Stack({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div {...props} className={classNames("ds-stack", className)} data-ds-component="stack" />;
}

export function GridCell({
  span,
  className,
  ...props
}: HTMLAttributes<HTMLDivElement> & { span: 3 | 4 | 6 | 8 | 12 }) {
  return (
    <div
      {...props}
      className={classNames("ds-grid-cell", className)}
      data-ds-component="grid-cell"
      data-span={span}
    />
  );
}

export function Text({
  tone = "default",
  size = "md",
  className,
  ...props
}: HTMLAttributes<HTMLParagraphElement> & {
  tone?: "default" | "muted";
  size?: "sm" | "md";
}) {
  return (
    <p
      {...props}
      className={classNames("ds-text", className)}
      data-ds-component="text"
      data-tone={tone}
      data-size={size}
    />
  );
}

export function Surface({
  padding = "md",
  className,
  ...props
}: HTMLAttributes<HTMLElement> & { padding?: "none" | "md" }) {
  return (
    <section
      {...props}
      className={classNames("ds-surface", className)}
      data-ds-component="surface"
      data-padding={padding}
    />
  );
}

export function Button({
  variant = "default",
  className,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "default" | "quiet" }) {
  return (
    <button
      {...props}
      type={type}
      className={classNames("ds-button", className)}
      data-ds-component="button"
      data-variant={variant}
    />
  );
}

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input {...props} className={classNames("ds-input", className)} data-ds-component="input" />
  );
}

export function TableViewport({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      {...props}
      className={classNames("ds-table-viewport", className)}
      data-ds-component="table-viewport"
    />
  );
}

export function Table({ className, ...props }: TableHTMLAttributes<HTMLTableElement>) {
  return (
    <table {...props} className={classNames("ds-table", className)} data-ds-component="table" />
  );
}
