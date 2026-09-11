"""Generate schemas and field documentation from the canonical proto package."""
from pathlib import Path
import json
import copy
import shutil
import sys
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / '.dependencies'))
from proto_contracts import load_contracts
D, CONFIGS = load_contracts(ROOT)

def main():
    target=ROOT/'contracts'; target.mkdir(parents=True,exist_ok=True)
    # Core types come from proto; package models remain separate extensions.
    core=copy.deepcopy(D)
    extensions=target/'extensions'; extensions.mkdir(exist_ok=True)
    for stale in extensions.glob('*.schema.json'):
        stale.unlink()
    def external_refs(value):
        if isinstance(value,dict):
            return {k:(f'uenv://schemas/vnext/{v.split("/")[-1]}' if v.split('/')[-1] in CONFIGS else 'urn:uenv:vnext:contracts'+v)
                    if k=='$ref' and v.startswith('#/$defs/') else external_refs(v) for k,v in value.items()}
        if isinstance(value,list): return [external_refs(x) for x in value]
        return value
    for name in CONFIGS:
        document={"$schema":"https://json-schema.org/draft/2020-12/schema", "$id":f"uenv://schemas/vnext/{name}", **external_refs(core.pop(name))}
        (extensions/(name+'.schema.json')).write_text(json.dumps(document,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    schema={"$schema":"https://json-schema.org/draft/2020-12/schema", "$id":"urn:uenv:vnext:contracts", "$defs":external_refs(core)}
    (target/'uenv.schema.json').write_text(json.dumps(schema,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    resources = ROOT/'reference/sdk/src/uenv/sdk/resources'
    resources.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(target/'uenv.schema.json', resources/'uenv.schema.json')
    shutil.copytree(extensions, resources/'extensions', dirs_exist_ok=True)
    for stale in (resources/'extensions').glob('*.schema.json'):
        if not (extensions/stale.name).exists():
            stale.unlink()
    lines=['# UEnv vNext 字段字典','', '本字典对应统一 step(tool_call) 参考契约；工具公开反馈统一在 ToolResult.observation 中。工具接口协商已删除：原生工具由 AgentManifest.provided_tools 导出，ResolvedToolBinding.native_agent 校验所属 Agent；SDK/MCP 接入方式由 Agent 实现决定。迁移范围见[工具接入与验收](../development/source_refactoring_plan.md#113-工具接入与验收)。', '', '本文件由 scripts/build_contracts.py 从 contracts/proto/uenv/v1/*.proto 生成；公共字段只修改 proto，然后重新生成 Rust/Python 类型、schema 和本字典，禁止手工编辑生成物。所有对象默认拒绝未知字段；表中的必填性针对完整传输对象；标注的默认值只由 Bridge 提交入口补齐，Server/Worker 校验不补值。run.yaml 展开后的完整 RunSpec 是唯一用户配置输入。', '',
           '嵌套字段的必填是指其父对象已提供时；可选父对象省略时无需补子字段。', '',
           'JSON 数字不得为 NaN/Infinity；时间统一毫秒。未提供的可选字段省略，不用空字符串代替 null；有明确允许空字符串的字段以定义为准。', '',
           'TypedConfig 的 data 不是任意 JSON：必须递归满足 schema_ref 指向的版本化 schema。表中 $ref 继续展开到同名结构。新扩展只在包内 models.py 定义，由发布工具生成并注册 schema，不在主流程增加名称分支。', '']
    for n,s in {**core, **{n:D[n] for n in CONFIGS}}.items():
        lines.extend([f'## {n}', '', s.get('description',''), ''])
        if 'properties' not in s:
            lines.extend(['联合类型：'+', '.join(v['$ref'].split('/')[-1] for v in s.get('oneOf',[])), '']); continue
        lines.extend(['| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |','|---|---|---|---|'])
        def field_rows(schema, prefix=''):
            for field, value in schema.get('properties', {}).items():
                yield prefix+field, value, field in schema.get('required', [])
                if value.get('type') == 'object':
                    yield from field_rows(value, prefix+field+'.')
                elif value.get('type') == 'array' and value.get('items', {}).get('type') == 'object':
                    yield from field_rows(value['items'], prefix+field+'[].')
        for k,v,required in field_rows(s):
            typ=v.get('$ref','').split('/')[-1] or str(v.get('type','按 discriminator 选择'))
            if v.get('type')=='array': typ='array<'+(v['items'].get('$ref','').split('/')[-1] or str(v['items'].get('type','value')))+'>'
            detail=v.get('description','')
            if 'enum' in v: detail+='；枚举：'+', '.join(map(str,v['enum']))
            if 'const' in v: detail+='；固定值：'+str(v['const'])
            if 'minimum' in v: detail+='；最小值：'+str(v['minimum'])
            if 'default' in v: detail+='；提交默认值：'+json.dumps(v['default'],ensure_ascii=False)
            lines.append(f"| `{k}` | {typ} | {'是' if required else '否'} | {detail.replace('|','/')} |")
        lines.append('')
    (ROOT/'docs/generated').mkdir(parents=True, exist_ok=True)
    (ROOT/'docs/generated/field_dictionary.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
    print(f'Generated {len(core)} core types and {len(CONFIGS)} separately registered extension schemas')

if __name__=='__main__': main()
