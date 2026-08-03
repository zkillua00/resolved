(() => {
  "use strict";

  // QuickJS intentionally does not ship ECMA-402. TypeScript only needs a
  // collator here to keep completion ordering stable.
  if (typeof globalThis.Intl === "undefined") {
    globalThis.Intl = {};
  }
  if (typeof globalThis.Intl.Collator !== "function") {
    globalThis.Intl.Collator = class Collator {
      compare(left, right) {
        const lhs = String(left);
        const rhs = String(right);
        return lhs < rhs ? -1 : lhs > rhs ? 1 : 0;
      }
    };
  }

  const typescript = globalThis.ts;
  if (!typescript || typeof typescript.createLanguageService !== "function") {
    throw new Error("the embedded TypeScript LanguageService is unavailable");
  }
  // All projects share the same virtual root and compatible standard-library
  // settings. Reusing one registry lets TypeScript retain a single AST for
  // each bundled library instead of six copies under QuickJS's cap.
  const documentRegistry = typescript.createDocumentRegistry(true, "/");

  const libraryEntries = JSON.parse(globalThis.__RESOLVED_TS_LIBRARIES_JSON);
  const runtimeDeclarations = globalThis.__RESOLVED_TS_RUNTIME_DTS;
  const phaseDeclarations = {
    pre: globalThis.__RESOLVED_TS_PRE_DTS,
    post: globalThis.__RESOLVED_TS_POST_DTS,
  };
  const snippetGeneratorDeclarations =
    globalThis.__RESOLVED_TS_SNIPPET_GENERATOR_DTS;
  const executableSnippetPhaseDeclarations = {
    pre: globalThis.__RESOLVED_TS_EXECUTABLE_SNIPPET_PRE_DTS,
    post: globalThis.__RESOLVED_TS_EXECUTABLE_SNIPPET_POST_DTS,
  };

  delete globalThis.__RESOLVED_TS_LIBRARIES_JSON;
  delete globalThis.__RESOLVED_TS_RUNTIME_DTS;
  delete globalThis.__RESOLVED_TS_PRE_DTS;
  delete globalThis.__RESOLVED_TS_POST_DTS;
  delete globalThis.__RESOLVED_TS_SNIPPET_GENERATOR_DTS;
  delete globalThis.__RESOLVED_TS_EXECUTABLE_SNIPPET_PRE_DTS;
  delete globalThis.__RESOLVED_TS_EXECUTABLE_SNIPPET_POST_DTS;

  function createProject(
    scriptFile,
    declarationEntries,
    strictNullChecks = false,
  ) {
    const files = new Map(libraryEntries);
    const versions = new Map();

    files.set("/resolved-runtime.d.ts", runtimeDeclarations);
    for (const [fileName, declarations] of declarationEntries) {
      files.set(fileName, declarations);
    }
    files.set(scriptFile, "");
    for (const fileName of files.keys()) {
      versions.set(fileName, "0");
    }

    const host = {
      getCompilationSettings: () => ({
        allowJs: true,
        allowNonTsExtensions: true,
        checkJs: true,
        libReplacement: false,
        module: typescript.ModuleKind.ESNext,
        noEmit: true,
        noLib: true,
        noImplicitAny: false,
        skipLibCheck: true,
        strict: false,
        strictNullChecks,
        target: typescript.ScriptTarget.ES2022,
        types: [],
      }),
      // Rust supplies only declarations implemented by the script runtime.
      // Make those files roots because the hostless service cannot resolve
      // triple-slash `lib` references through TypeScript's filesystem helper.
      getScriptFileNames: () => Array.from(files.keys()),
      getScriptVersion: (fileName) => versions.get(fileName) || "0",
      getScriptSnapshot: (fileName) => {
        const source = files.get(fileName);
        return source === undefined
          ? undefined
          : typescript.ScriptSnapshot.fromString(source);
      },
      getScriptKind: (fileName) =>
        fileName === scriptFile ? typescript.ScriptKind.JS : typescript.ScriptKind.TS,
      getCurrentDirectory: () => "/",
      getDefaultLibFileName: () => "/lib.es2022.d.ts",
      getDefaultLibLocation: () => "/",
      getNewLine: () => "\n",
      getProjectVersion: () => versions.get(scriptFile) || "0",
      useCaseSensitiveFileNames: () => true,
      fileExists: (fileName) => files.has(fileName),
      readFile: (fileName) => files.get(fileName),
      readDirectory: () => [],
      directoryExists: (directoryName) => directoryName === "/",
      getDirectories: () => [],
      realpath: (fileName) => fileName,
    };

    return {
      scriptFile,
      files,
      versions,
      service: typescript.createLanguageService(
        host,
        documentRegistry,
      ),
    };
  }

  const projects = {
    "script-pre": createProject("/script-pre.js", [
      ["/pre-request.d.ts", phaseDeclarations.pre],
    ]),
    "script-post": createProject("/script-post.js", [
      ["/post-response.d.ts", phaseDeclarations.post],
    ]),
    "plain-snippet-pre": createProject("/plain-snippet-pre.js", [
      ["/pre-request.d.ts", phaseDeclarations.pre],
    ]),
    "plain-snippet-post": createProject("/plain-snippet-post.js", [
      ["/post-response.d.ts", phaseDeclarations.post],
    ]),
    "executable-snippet-pre": createProject(
      "/executable-snippet-pre.js",
      [
        ["/snippet-generator.d.ts", snippetGeneratorDeclarations],
        [
          "/executable-snippet-pre-request.d.ts",
          executableSnippetPhaseDeclarations.pre,
        ],
      ],
      true,
    ),
    "executable-snippet-post": createProject(
      "/executable-snippet-post.js",
      [
        ["/snippet-generator.d.ts", snippetGeneratorDeclarations],
        [
          "/executable-snippet-post-response.d.ts",
          executableSnippetPhaseDeclarations.post,
        ],
      ],
      true,
    ),
  };

  function projectFor(documentKind) {
    const project = projects[documentKind];
    if (!project) {
      throw new TypeError(`unknown TypeScript document kind: ${documentKind}`);
    }
    return project;
  }

  function updateDocument(documentKind, source, version) {
    const project = projectFor(documentKind);
    project.files.set(project.scriptFile, String(source));
    project.versions.set(project.scriptFile, String(version));
  }

  function identifierReplacementSpan(project, offset) {
    const source = project.files.get(project.scriptFile) || "";
    let start = offset;
    while (start > 0) {
      let previous = start - 1;
      const trailingUnit = source.charCodeAt(previous);
      if (trailingUnit >= 0xdc00 && trailingUnit <= 0xdfff && previous > 0) {
        const leadingUnit = source.charCodeAt(previous - 1);
        if (leadingUnit >= 0xd800 && leadingUnit <= 0xdbff) {
          previous -= 1;
        }
      }
      const codePoint = source.codePointAt(previous);
      if (!typescript.isIdentifierPart(codePoint, typescript.ScriptTarget.ES2022)) {
        break;
      }
      start = previous;
    }
    return { start, length: offset - start };
  }

  function completions(documentKind, offset) {
    const project = projectFor(documentKind);
    const result = project.service.getCompletionsAtPosition(
      project.scriptFile,
      offset,
      {
        allowIncompleteCompletions: true,
        includeCompletionsForImportStatements: false,
        includeCompletionsForModuleExports: false,
        includeCompletionsWithClassMemberSnippets: true,
        includeCompletionsWithInsertText: true,
        includeInsertTextCompletions: true,
      },
    );
    if (!result) {
      return "[]";
    }
    const defaultReplacementSpan = result.optionalReplacementSpan || null;
    const fallbackReplacementSpan = identifierReplacementSpan(project, offset);
    return JSON.stringify(
      result.entries.map((entry) => {
        const replacementSpan =
          entry.replacementSpan || defaultReplacementSpan || fallbackReplacementSpan;
        return {
          name: entry.name,
          kind: entry.kind,
          kindModifiers: entry.kindModifiers,
          sortText: entry.sortText,
          insertText: entry.insertText,
          isSnippet: entry.isSnippet === true,
          source: entry.source,
          replacementSpan: {
            start: replacementSpan.start,
            length: replacementSpan.length,
          },
        };
      }),
    );
  }

  function hover(documentKind, offset) {
    const project = projectFor(documentKind);
    const info = project.service.getQuickInfoAtPosition(project.scriptFile, offset);
    if (!info) {
      return "null";
    }
    const documentation = typescript.displayPartsToString(info.documentation || []);
    const tags = (info.tags || [])
      .map((tag) => {
        const text = Array.isArray(tag.text)
          ? typescript.displayPartsToString(tag.text)
          : String(tag.text || "");
        return text ? `@${tag.name} ${text}` : `@${tag.name}`;
      })
      .join("\n");
    return JSON.stringify({
      start: info.textSpan.start,
      length: info.textSpan.length,
      display: typescript.displayPartsToString(info.displayParts || []),
      documentation: tags
        ? documentation
          ? `${documentation}\n\n${tags}`
          : tags
        : documentation,
    });
  }

  function runtimeContractDiagnostics(project) {
    const program = project.service.getProgram();
    const sourceFile = program?.getSourceFile(project.scriptFile);
    if (!program || !sourceFile) {
      return [];
    }
    const checker = program.getTypeChecker();

    const diagnostics = [];
    const push = (node, category, code, messageText) => {
      diagnostics.push({
        start: node.getStart(sourceFile),
        length: Math.max(1, node.getWidth(sourceFile)),
        category,
        code,
        messageText,
      });
    };
    const isReferenceIdentifier = (node) => {
      if (!typescript.isIdentifier(node) || typescript.isDeclarationName(node)) {
        return false;
      }
      const parent = node.parent;
      if (typescript.isPropertyAccessExpression(parent) && parent.name === node) {
        return false;
      }
      return true;
    };
    const resolvesToLocalDeclaration = (node) => {
      const symbol = checker.getSymbolAtLocation(node);
      return (
        symbol?.declarations?.some(
          (declaration) => declaration.getSourceFile() === sourceFile,
        ) === true
      );
    };
    const isRuntimeGlobalReference = (node, name) =>
      isReferenceIdentifier(node) &&
      node.text === name &&
      !resolvesToLocalDeclaration(node);

    const visit = (node) => {
      if (isRuntimeGlobalReference(node, "Intl")) {
        push(
          node,
          typescript.DiagnosticCategory.Error,
          90001,
          "Intl is unavailable in the Resolved QuickJS runtime.",
        );
      }
      if (
        typescript.isPropertyAccessExpression(node) &&
        typescript.isIdentifier(node.expression) &&
        node.expression.text === "globalThis" &&
        !resolvesToLocalDeclaration(node.expression) &&
        node.name.text === "Intl"
      ) {
        push(
          node.name,
          typescript.DiagnosticCategory.Error,
          90001,
          "Intl is unavailable in the Resolved QuickJS runtime.",
        );
      }
      if (isRuntimeGlobalReference(node, "Promise")) {
        push(
          node,
          typescript.DiagnosticCategory.Warning,
          90002,
          "Resolved scripts do not drain queued Promise jobs; asynchronous effects will not run.",
        );
      }
      const asyncModifier = node.modifiers?.find(
        (modifier) => modifier.kind === typescript.SyntaxKind.AsyncKeyword,
      );
      if (asyncModifier) {
        push(
          asyncModifier,
          typescript.DiagnosticCategory.Warning,
          90002,
          "Resolved scripts run synchronously and do not await asynchronous functions.",
        );
      }
      typescript.forEachChild(node, visit);
    };
    visit(sourceFile);
    return diagnostics;
  }

  function diagnostics(documentKind) {
    const project = projectFor(documentKind);
    const diagnostics = [
      ...project.service.getSyntacticDiagnostics(project.scriptFile),
      ...project.service.getSemanticDiagnostics(project.scriptFile),
      ...runtimeContractDiagnostics(project),
    ];
    return JSON.stringify(
      diagnostics.map((diagnostic) => ({
        start: diagnostic.start || 0,
        length: diagnostic.length || 0,
        category: diagnostic.category,
        code: diagnostic.code,
        message: typescript.flattenDiagnosticMessageText(diagnostic.messageText, "\n"),
      })),
    );
  }

  globalThis.__resolvedTypeScriptService = Object.freeze({
    completions,
    diagnostics,
    hover,
    updateDocument,
  });
})();
