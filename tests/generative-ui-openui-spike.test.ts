import { describe, expect, test } from 'bun:test';
import { validateRendererDefinition, uiLibrary } from '../scripts/generative-ui-openui-spike';
import { createParser } from '@openuidev/react-lang';

describe('bounded OpenUI renderer contract', () => {
  test('parses dashboard references and all ten registered components', () => {
    const result = validateRendererDefinition('root = Grid([Cell(models,8),Cell(metric,4)]);\nmodels = ModelStatus("larm.status");\nmetric = Metric("runtime.summary","running","実行中")');
    expect(result.root?.typeName).toBe('Grid');
    expect(result.meta.errors).toEqual([]);
    expect(Object.keys(uiLibrary.components)).toHaveLength(10);
    for (const definition of [
      'root = Stack([Text("状態"),Table("runtime.runs","provider,status"),Chart("runtime.history","time","count"),Actions("refresh"),Status("runtime.summary","running","実行中")])',
      'root = Text("日本語と\\n改行")',
    ]) expect(() => validateRendererDefinition(definition)).not.toThrow();
  });
  test('rejects incomplete code and runtime tools even if the external parser recognizes them', () => {
    for (const definition of ['root=Unknown("x")','root=Text(','root=Grid([missing])','data=Query("anything",{},{}); root=Text("x")','$x = "value"; root=Text("x")','m=Mutation("anything",{}); root=Text("x")']) {
      expect(() => validateRendererDefinition(definition)).toThrow();
    }
  });
  test('catches dropped excess positional arguments', () => {
    const result = createParser(uiLibrary.toJSONSchema()).parse('root=Text("a","b")');
    expect(result.meta.errors.length).toBeGreaterThan(0);
    expect(() => validateRendererDefinition('root=Text("a","b")')).toThrow();
  });
});
