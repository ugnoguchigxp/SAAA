import { useState } from "react";
import { stewardApi } from "./stewardApi";

export function TestRecipeForm({ onRegistered }: { onRegistered: () => void }) {
  const [name, setName] = useState("unit");
  const [target, setTarget] = useState("tests");
  const [argv, setArgv] = useState("/usr/bin/true");
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void stewardApi
          .registerRecipe({
            name,
            target,
            cwd: ".",
            argv: argv.split(" ").filter(Boolean),
            envAllow: [],
            outputDir: "/tmp",
            timeoutMs: 60_000,
          })
          .then(onRegistered);
      }}
    >
      <label>
        レシピ名
        <input value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <label>
        対象
        <input value={target} onChange={(event) => setTarget(event.target.value)} />
      </label>
      <label>
        確認済みコマンド
        <input value={argv} onChange={(event) => setArgv(event.target.value)} />
      </label>
      <button type="submit">テストレシピを登録</button>
    </form>
  );
}
