import {
  require_abci,
  require_any,
  require_binary,
  require_build,
  require_build2,
  require_build3,
  require_build4,
  require_build5,
  require_build6,
  require_build7,
  require_build8,
  require_coin,
  require_helpers,
  require_pagination,
  require_signing,
  require_tx,
  require_tx2,
  require_tx3
} from "./chunk-USLIDOQD.js";
import {
  __commonJS,
  __publicField
} from "./chunk-EWTE5DHJ.js";

// node_modules/cosmjs-types/cosmwasm/wasm/v1/types.js
var require_types = __commonJS({
  "node_modules/cosmjs-types/cosmwasm/wasm/v1/types.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.Model = exports.AbsoluteTxPosition = exports.ContractCodeHistoryEntry = exports.ContractInfo = exports.CodeInfo = exports.Params = exports.AccessConfig = exports.AccessTypeParam = exports.contractCodeHistoryOperationTypeToJSON = exports.contractCodeHistoryOperationTypeFromJSON = exports.ContractCodeHistoryOperationType = exports.accessTypeToJSON = exports.accessTypeFromJSON = exports.AccessType = exports.protobufPackage = void 0;
    var any_1 = require_any();
    var binary_1 = require_binary();
    var helpers_1 = require_helpers();
    exports.protobufPackage = "cosmwasm.wasm.v1";
    var AccessType;
    (function(AccessType2) {
      AccessType2[AccessType2["ACCESS_TYPE_UNSPECIFIED"] = 0] = "ACCESS_TYPE_UNSPECIFIED";
      AccessType2[AccessType2["ACCESS_TYPE_NOBODY"] = 1] = "ACCESS_TYPE_NOBODY";
      AccessType2[AccessType2["ACCESS_TYPE_EVERYBODY"] = 3] = "ACCESS_TYPE_EVERYBODY";
      AccessType2[AccessType2["ACCESS_TYPE_ANY_OF_ADDRESSES"] = 4] = "ACCESS_TYPE_ANY_OF_ADDRESSES";
      AccessType2[AccessType2["UNRECOGNIZED"] = -1] = "UNRECOGNIZED";
    })(AccessType || (exports.AccessType = AccessType = {}));
    function accessTypeFromJSON(object) {
      switch (object) {
        case 0:
        case "ACCESS_TYPE_UNSPECIFIED":
          return AccessType.ACCESS_TYPE_UNSPECIFIED;
        case 1:
        case "ACCESS_TYPE_NOBODY":
          return AccessType.ACCESS_TYPE_NOBODY;
        case 3:
        case "ACCESS_TYPE_EVERYBODY":
          return AccessType.ACCESS_TYPE_EVERYBODY;
        case 4:
        case "ACCESS_TYPE_ANY_OF_ADDRESSES":
          return AccessType.ACCESS_TYPE_ANY_OF_ADDRESSES;
        case -1:
        case "UNRECOGNIZED":
        default:
          return AccessType.UNRECOGNIZED;
      }
    }
    exports.accessTypeFromJSON = accessTypeFromJSON;
    function accessTypeToJSON(object) {
      switch (object) {
        case AccessType.ACCESS_TYPE_UNSPECIFIED:
          return "ACCESS_TYPE_UNSPECIFIED";
        case AccessType.ACCESS_TYPE_NOBODY:
          return "ACCESS_TYPE_NOBODY";
        case AccessType.ACCESS_TYPE_EVERYBODY:
          return "ACCESS_TYPE_EVERYBODY";
        case AccessType.ACCESS_TYPE_ANY_OF_ADDRESSES:
          return "ACCESS_TYPE_ANY_OF_ADDRESSES";
        case AccessType.UNRECOGNIZED:
        default:
          return "UNRECOGNIZED";
      }
    }
    exports.accessTypeToJSON = accessTypeToJSON;
    var ContractCodeHistoryOperationType;
    (function(ContractCodeHistoryOperationType2) {
      ContractCodeHistoryOperationType2[ContractCodeHistoryOperationType2["CONTRACT_CODE_HISTORY_OPERATION_TYPE_UNSPECIFIED"] = 0] = "CONTRACT_CODE_HISTORY_OPERATION_TYPE_UNSPECIFIED";
      ContractCodeHistoryOperationType2[ContractCodeHistoryOperationType2["CONTRACT_CODE_HISTORY_OPERATION_TYPE_INIT"] = 1] = "CONTRACT_CODE_HISTORY_OPERATION_TYPE_INIT";
      ContractCodeHistoryOperationType2[ContractCodeHistoryOperationType2["CONTRACT_CODE_HISTORY_OPERATION_TYPE_MIGRATE"] = 2] = "CONTRACT_CODE_HISTORY_OPERATION_TYPE_MIGRATE";
      ContractCodeHistoryOperationType2[ContractCodeHistoryOperationType2["CONTRACT_CODE_HISTORY_OPERATION_TYPE_GENESIS"] = 3] = "CONTRACT_CODE_HISTORY_OPERATION_TYPE_GENESIS";
      ContractCodeHistoryOperationType2[ContractCodeHistoryOperationType2["UNRECOGNIZED"] = -1] = "UNRECOGNIZED";
    })(ContractCodeHistoryOperationType || (exports.ContractCodeHistoryOperationType = ContractCodeHistoryOperationType = {}));
    function contractCodeHistoryOperationTypeFromJSON(object) {
      switch (object) {
        case 0:
        case "CONTRACT_CODE_HISTORY_OPERATION_TYPE_UNSPECIFIED":
          return ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_UNSPECIFIED;
        case 1:
        case "CONTRACT_CODE_HISTORY_OPERATION_TYPE_INIT":
          return ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_INIT;
        case 2:
        case "CONTRACT_CODE_HISTORY_OPERATION_TYPE_MIGRATE":
          return ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_MIGRATE;
        case 3:
        case "CONTRACT_CODE_HISTORY_OPERATION_TYPE_GENESIS":
          return ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_GENESIS;
        case -1:
        case "UNRECOGNIZED":
        default:
          return ContractCodeHistoryOperationType.UNRECOGNIZED;
      }
    }
    exports.contractCodeHistoryOperationTypeFromJSON = contractCodeHistoryOperationTypeFromJSON;
    function contractCodeHistoryOperationTypeToJSON(object) {
      switch (object) {
        case ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_UNSPECIFIED:
          return "CONTRACT_CODE_HISTORY_OPERATION_TYPE_UNSPECIFIED";
        case ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_INIT:
          return "CONTRACT_CODE_HISTORY_OPERATION_TYPE_INIT";
        case ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_MIGRATE:
          return "CONTRACT_CODE_HISTORY_OPERATION_TYPE_MIGRATE";
        case ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_GENESIS:
          return "CONTRACT_CODE_HISTORY_OPERATION_TYPE_GENESIS";
        case ContractCodeHistoryOperationType.UNRECOGNIZED:
        default:
          return "UNRECOGNIZED";
      }
    }
    exports.contractCodeHistoryOperationTypeToJSON = contractCodeHistoryOperationTypeToJSON;
    function createBaseAccessTypeParam() {
      return {
        value: 0
      };
    }
    exports.AccessTypeParam = {
      typeUrl: "/cosmwasm.wasm.v1.AccessTypeParam",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.value !== 0) {
          writer.uint32(8).int32(message.value);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseAccessTypeParam();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.value = reader.int32();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseAccessTypeParam();
        if ((0, helpers_1.isSet)(object.value))
          obj.value = accessTypeFromJSON(object.value);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.value !== void 0 && (obj.value = accessTypeToJSON(message.value));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseAccessTypeParam();
        message.value = object.value ?? 0;
        return message;
      }
    };
    function createBaseAccessConfig() {
      return {
        permission: 0,
        addresses: []
      };
    }
    exports.AccessConfig = {
      typeUrl: "/cosmwasm.wasm.v1.AccessConfig",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.permission !== 0) {
          writer.uint32(8).int32(message.permission);
        }
        for (const v of message.addresses) {
          writer.uint32(26).string(v);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseAccessConfig();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.permission = reader.int32();
              break;
            case 3:
              message.addresses.push(reader.string());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseAccessConfig();
        if ((0, helpers_1.isSet)(object.permission))
          obj.permission = accessTypeFromJSON(object.permission);
        if (Array.isArray(object == null ? void 0 : object.addresses))
          obj.addresses = object.addresses.map((e) => String(e));
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.permission !== void 0 && (obj.permission = accessTypeToJSON(message.permission));
        if (message.addresses) {
          obj.addresses = message.addresses.map((e) => e);
        } else {
          obj.addresses = [];
        }
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseAccessConfig();
        message.permission = object.permission ?? 0;
        message.addresses = ((_a = object.addresses) == null ? void 0 : _a.map((e) => e)) || [];
        return message;
      }
    };
    function createBaseParams() {
      return {
        codeUploadAccess: exports.AccessConfig.fromPartial({}),
        instantiateDefaultPermission: 0
      };
    }
    exports.Params = {
      typeUrl: "/cosmwasm.wasm.v1.Params",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeUploadAccess !== void 0) {
          exports.AccessConfig.encode(message.codeUploadAccess, writer.uint32(10).fork()).ldelim();
        }
        if (message.instantiateDefaultPermission !== 0) {
          writer.uint32(16).int32(message.instantiateDefaultPermission);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseParams();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeUploadAccess = exports.AccessConfig.decode(reader, reader.uint32());
              break;
            case 2:
              message.instantiateDefaultPermission = reader.int32();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseParams();
        if ((0, helpers_1.isSet)(object.codeUploadAccess))
          obj.codeUploadAccess = exports.AccessConfig.fromJSON(object.codeUploadAccess);
        if ((0, helpers_1.isSet)(object.instantiateDefaultPermission))
          obj.instantiateDefaultPermission = accessTypeFromJSON(object.instantiateDefaultPermission);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeUploadAccess !== void 0 && (obj.codeUploadAccess = message.codeUploadAccess ? exports.AccessConfig.toJSON(message.codeUploadAccess) : void 0);
        message.instantiateDefaultPermission !== void 0 && (obj.instantiateDefaultPermission = accessTypeToJSON(message.instantiateDefaultPermission));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseParams();
        if (object.codeUploadAccess !== void 0 && object.codeUploadAccess !== null) {
          message.codeUploadAccess = exports.AccessConfig.fromPartial(object.codeUploadAccess);
        }
        message.instantiateDefaultPermission = object.instantiateDefaultPermission ?? 0;
        return message;
      }
    };
    function createBaseCodeInfo() {
      return {
        codeHash: new Uint8Array(),
        creator: "",
        instantiateConfig: exports.AccessConfig.fromPartial({})
      };
    }
    exports.CodeInfo = {
      typeUrl: "/cosmwasm.wasm.v1.CodeInfo",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeHash.length !== 0) {
          writer.uint32(10).bytes(message.codeHash);
        }
        if (message.creator !== "") {
          writer.uint32(18).string(message.creator);
        }
        if (message.instantiateConfig !== void 0) {
          exports.AccessConfig.encode(message.instantiateConfig, writer.uint32(42).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseCodeInfo();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeHash = reader.bytes();
              break;
            case 2:
              message.creator = reader.string();
              break;
            case 5:
              message.instantiateConfig = exports.AccessConfig.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseCodeInfo();
        if ((0, helpers_1.isSet)(object.codeHash))
          obj.codeHash = (0, helpers_1.bytesFromBase64)(object.codeHash);
        if ((0, helpers_1.isSet)(object.creator))
          obj.creator = String(object.creator);
        if ((0, helpers_1.isSet)(object.instantiateConfig))
          obj.instantiateConfig = exports.AccessConfig.fromJSON(object.instantiateConfig);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeHash !== void 0 && (obj.codeHash = (0, helpers_1.base64FromBytes)(message.codeHash !== void 0 ? message.codeHash : new Uint8Array()));
        message.creator !== void 0 && (obj.creator = message.creator);
        message.instantiateConfig !== void 0 && (obj.instantiateConfig = message.instantiateConfig ? exports.AccessConfig.toJSON(message.instantiateConfig) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseCodeInfo();
        message.codeHash = object.codeHash ?? new Uint8Array();
        message.creator = object.creator ?? "";
        if (object.instantiateConfig !== void 0 && object.instantiateConfig !== null) {
          message.instantiateConfig = exports.AccessConfig.fromPartial(object.instantiateConfig);
        }
        return message;
      }
    };
    function createBaseContractInfo() {
      return {
        codeId: BigInt(0),
        creator: "",
        admin: "",
        label: "",
        created: void 0,
        ibcPortId: "",
        extension: void 0
      };
    }
    exports.ContractInfo = {
      typeUrl: "/cosmwasm.wasm.v1.ContractInfo",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeId !== BigInt(0)) {
          writer.uint32(8).uint64(message.codeId);
        }
        if (message.creator !== "") {
          writer.uint32(18).string(message.creator);
        }
        if (message.admin !== "") {
          writer.uint32(26).string(message.admin);
        }
        if (message.label !== "") {
          writer.uint32(34).string(message.label);
        }
        if (message.created !== void 0) {
          exports.AbsoluteTxPosition.encode(message.created, writer.uint32(42).fork()).ldelim();
        }
        if (message.ibcPortId !== "") {
          writer.uint32(50).string(message.ibcPortId);
        }
        if (message.extension !== void 0) {
          any_1.Any.encode(message.extension, writer.uint32(58).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseContractInfo();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeId = reader.uint64();
              break;
            case 2:
              message.creator = reader.string();
              break;
            case 3:
              message.admin = reader.string();
              break;
            case 4:
              message.label = reader.string();
              break;
            case 5:
              message.created = exports.AbsoluteTxPosition.decode(reader, reader.uint32());
              break;
            case 6:
              message.ibcPortId = reader.string();
              break;
            case 7:
              message.extension = any_1.Any.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseContractInfo();
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.creator))
          obj.creator = String(object.creator);
        if ((0, helpers_1.isSet)(object.admin))
          obj.admin = String(object.admin);
        if ((0, helpers_1.isSet)(object.label))
          obj.label = String(object.label);
        if ((0, helpers_1.isSet)(object.created))
          obj.created = exports.AbsoluteTxPosition.fromJSON(object.created);
        if ((0, helpers_1.isSet)(object.ibcPortId))
          obj.ibcPortId = String(object.ibcPortId);
        if ((0, helpers_1.isSet)(object.extension))
          obj.extension = any_1.Any.fromJSON(object.extension);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.creator !== void 0 && (obj.creator = message.creator);
        message.admin !== void 0 && (obj.admin = message.admin);
        message.label !== void 0 && (obj.label = message.label);
        message.created !== void 0 && (obj.created = message.created ? exports.AbsoluteTxPosition.toJSON(message.created) : void 0);
        message.ibcPortId !== void 0 && (obj.ibcPortId = message.ibcPortId);
        message.extension !== void 0 && (obj.extension = message.extension ? any_1.Any.toJSON(message.extension) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseContractInfo();
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        message.creator = object.creator ?? "";
        message.admin = object.admin ?? "";
        message.label = object.label ?? "";
        if (object.created !== void 0 && object.created !== null) {
          message.created = exports.AbsoluteTxPosition.fromPartial(object.created);
        }
        message.ibcPortId = object.ibcPortId ?? "";
        if (object.extension !== void 0 && object.extension !== null) {
          message.extension = any_1.Any.fromPartial(object.extension);
        }
        return message;
      }
    };
    function createBaseContractCodeHistoryEntry() {
      return {
        operation: 0,
        codeId: BigInt(0),
        updated: void 0,
        msg: new Uint8Array()
      };
    }
    exports.ContractCodeHistoryEntry = {
      typeUrl: "/cosmwasm.wasm.v1.ContractCodeHistoryEntry",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.operation !== 0) {
          writer.uint32(8).int32(message.operation);
        }
        if (message.codeId !== BigInt(0)) {
          writer.uint32(16).uint64(message.codeId);
        }
        if (message.updated !== void 0) {
          exports.AbsoluteTxPosition.encode(message.updated, writer.uint32(26).fork()).ldelim();
        }
        if (message.msg.length !== 0) {
          writer.uint32(34).bytes(message.msg);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseContractCodeHistoryEntry();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.operation = reader.int32();
              break;
            case 2:
              message.codeId = reader.uint64();
              break;
            case 3:
              message.updated = exports.AbsoluteTxPosition.decode(reader, reader.uint32());
              break;
            case 4:
              message.msg = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseContractCodeHistoryEntry();
        if ((0, helpers_1.isSet)(object.operation))
          obj.operation = contractCodeHistoryOperationTypeFromJSON(object.operation);
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.updated))
          obj.updated = exports.AbsoluteTxPosition.fromJSON(object.updated);
        if ((0, helpers_1.isSet)(object.msg))
          obj.msg = (0, helpers_1.bytesFromBase64)(object.msg);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.operation !== void 0 && (obj.operation = contractCodeHistoryOperationTypeToJSON(message.operation));
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.updated !== void 0 && (obj.updated = message.updated ? exports.AbsoluteTxPosition.toJSON(message.updated) : void 0);
        message.msg !== void 0 && (obj.msg = (0, helpers_1.base64FromBytes)(message.msg !== void 0 ? message.msg : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseContractCodeHistoryEntry();
        message.operation = object.operation ?? 0;
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        if (object.updated !== void 0 && object.updated !== null) {
          message.updated = exports.AbsoluteTxPosition.fromPartial(object.updated);
        }
        message.msg = object.msg ?? new Uint8Array();
        return message;
      }
    };
    function createBaseAbsoluteTxPosition() {
      return {
        blockHeight: BigInt(0),
        txIndex: BigInt(0)
      };
    }
    exports.AbsoluteTxPosition = {
      typeUrl: "/cosmwasm.wasm.v1.AbsoluteTxPosition",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.blockHeight !== BigInt(0)) {
          writer.uint32(8).uint64(message.blockHeight);
        }
        if (message.txIndex !== BigInt(0)) {
          writer.uint32(16).uint64(message.txIndex);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseAbsoluteTxPosition();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.blockHeight = reader.uint64();
              break;
            case 2:
              message.txIndex = reader.uint64();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseAbsoluteTxPosition();
        if ((0, helpers_1.isSet)(object.blockHeight))
          obj.blockHeight = BigInt(object.blockHeight.toString());
        if ((0, helpers_1.isSet)(object.txIndex))
          obj.txIndex = BigInt(object.txIndex.toString());
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.blockHeight !== void 0 && (obj.blockHeight = (message.blockHeight || BigInt(0)).toString());
        message.txIndex !== void 0 && (obj.txIndex = (message.txIndex || BigInt(0)).toString());
        return obj;
      },
      fromPartial(object) {
        const message = createBaseAbsoluteTxPosition();
        if (object.blockHeight !== void 0 && object.blockHeight !== null) {
          message.blockHeight = BigInt(object.blockHeight.toString());
        }
        if (object.txIndex !== void 0 && object.txIndex !== null) {
          message.txIndex = BigInt(object.txIndex.toString());
        }
        return message;
      }
    };
    function createBaseModel() {
      return {
        key: new Uint8Array(),
        value: new Uint8Array()
      };
    }
    exports.Model = {
      typeUrl: "/cosmwasm.wasm.v1.Model",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.key.length !== 0) {
          writer.uint32(10).bytes(message.key);
        }
        if (message.value.length !== 0) {
          writer.uint32(18).bytes(message.value);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseModel();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.key = reader.bytes();
              break;
            case 2:
              message.value = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseModel();
        if ((0, helpers_1.isSet)(object.key))
          obj.key = (0, helpers_1.bytesFromBase64)(object.key);
        if ((0, helpers_1.isSet)(object.value))
          obj.value = (0, helpers_1.bytesFromBase64)(object.value);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.key !== void 0 && (obj.key = (0, helpers_1.base64FromBytes)(message.key !== void 0 ? message.key : new Uint8Array()));
        message.value !== void 0 && (obj.value = (0, helpers_1.base64FromBytes)(message.value !== void 0 ? message.value : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseModel();
        message.key = object.key ?? new Uint8Array();
        message.value = object.value ?? new Uint8Array();
        return message;
      }
    };
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/modules/wasm/aminomessages.js
var require_aminomessages = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/modules/wasm/aminomessages.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.accessTypeFromString = accessTypeFromString;
    exports.accessTypeToString = accessTypeToString;
    exports.createWasmAminoConverters = createWasmAminoConverters;
    var amino_1 = require_build5();
    var encoding_1 = require_build3();
    var types_1 = require_types();
    function accessTypeFromString(str) {
      switch (str) {
        case "Unspecified":
          return types_1.AccessType.ACCESS_TYPE_UNSPECIFIED;
        case "Nobody":
          return types_1.AccessType.ACCESS_TYPE_NOBODY;
        case "Everybody":
          return types_1.AccessType.ACCESS_TYPE_EVERYBODY;
        case "AnyOfAddresses":
          return types_1.AccessType.ACCESS_TYPE_ANY_OF_ADDRESSES;
        default:
          return types_1.AccessType.UNRECOGNIZED;
      }
    }
    function accessTypeToString(object) {
      switch (object) {
        case types_1.AccessType.ACCESS_TYPE_UNSPECIFIED:
          return "Unspecified";
        case types_1.AccessType.ACCESS_TYPE_NOBODY:
          return "Nobody";
        case types_1.AccessType.ACCESS_TYPE_EVERYBODY:
          return "Everybody";
        case types_1.AccessType.ACCESS_TYPE_ANY_OF_ADDRESSES:
          return "AnyOfAddresses";
        case types_1.AccessType.UNRECOGNIZED:
        default:
          return "UNRECOGNIZED";
      }
    }
    function createWasmAminoConverters() {
      return {
        "/cosmwasm.wasm.v1.MsgStoreCode": {
          aminoType: "wasm/MsgStoreCode",
          toAmino: ({ sender, wasmByteCode, instantiatePermission }) => ({
            sender,
            wasm_byte_code: (0, encoding_1.toBase64)(wasmByteCode),
            instantiate_permission: instantiatePermission ? {
              permission: accessTypeToString(instantiatePermission.permission),
              addresses: instantiatePermission.addresses.length !== 0 ? instantiatePermission.addresses : void 0
            } : void 0
          }),
          fromAmino: ({ sender, wasm_byte_code, instantiate_permission }) => ({
            sender,
            wasmByteCode: (0, encoding_1.fromBase64)(wasm_byte_code),
            instantiatePermission: instantiate_permission ? types_1.AccessConfig.fromPartial({
              permission: accessTypeFromString(instantiate_permission.permission),
              addresses: instantiate_permission.addresses ?? []
            }) : void 0
          })
        },
        "/cosmwasm.wasm.v1.MsgInstantiateContract": {
          aminoType: "wasm/MsgInstantiateContract",
          toAmino: ({ sender, codeId, label, msg, funds, admin }) => ({
            sender,
            code_id: codeId.toString(),
            label,
            msg: JSON.parse((0, encoding_1.fromUtf8)(msg)),
            funds,
            admin: (0, amino_1.omitDefault)(admin)
          }),
          fromAmino: ({ sender, code_id, label, msg, funds, admin }) => ({
            sender,
            codeId: BigInt(code_id),
            label,
            msg: (0, encoding_1.toUtf8)(JSON.stringify(msg)),
            funds: [...funds],
            admin: admin ?? ""
          })
        },
        "/cosmwasm.wasm.v1.MsgInstantiateContract2": {
          aminoType: "wasm/MsgInstantiateContract2",
          toAmino: ({ sender, codeId, label, msg, funds, admin, salt, fixMsg }) => ({
            sender,
            code_id: codeId.toString(),
            label,
            msg: JSON.parse((0, encoding_1.fromUtf8)(msg)),
            funds,
            admin: (0, amino_1.omitDefault)(admin),
            salt: (0, encoding_1.toBase64)(salt),
            fix_msg: (0, amino_1.omitDefault)(fixMsg)
          }),
          fromAmino: ({ sender, code_id, label, msg, funds, admin, salt, fix_msg }) => ({
            sender,
            codeId: BigInt(code_id),
            label,
            msg: (0, encoding_1.toUtf8)(JSON.stringify(msg)),
            funds: [...funds],
            admin: admin ?? "",
            salt: (0, encoding_1.fromBase64)(salt),
            fixMsg: fix_msg ?? false
          })
        },
        "/cosmwasm.wasm.v1.MsgUpdateAdmin": {
          aminoType: "wasm/MsgUpdateAdmin",
          toAmino: ({ sender, newAdmin, contract }) => ({
            sender,
            new_admin: newAdmin,
            contract
          }),
          fromAmino: ({ sender, new_admin, contract }) => ({
            sender,
            newAdmin: new_admin,
            contract
          })
        },
        "/cosmwasm.wasm.v1.MsgClearAdmin": {
          aminoType: "wasm/MsgClearAdmin",
          toAmino: ({ sender, contract }) => ({
            sender,
            contract
          }),
          fromAmino: ({ sender, contract }) => ({
            sender,
            contract
          })
        },
        "/cosmwasm.wasm.v1.MsgExecuteContract": {
          aminoType: "wasm/MsgExecuteContract",
          toAmino: ({ sender, contract, msg, funds }) => ({
            sender,
            contract,
            msg: JSON.parse((0, encoding_1.fromUtf8)(msg)),
            funds
          }),
          fromAmino: ({ sender, contract, msg, funds }) => ({
            sender,
            contract,
            msg: (0, encoding_1.toUtf8)(JSON.stringify(msg)),
            funds: [...funds]
          })
        },
        "/cosmwasm.wasm.v1.MsgMigrateContract": {
          aminoType: "wasm/MsgMigrateContract",
          toAmino: ({ sender, contract, codeId, msg }) => ({
            sender,
            contract,
            code_id: codeId.toString(),
            msg: JSON.parse((0, encoding_1.fromUtf8)(msg))
          }),
          fromAmino: ({ sender, contract, code_id, msg }) => ({
            sender,
            contract,
            codeId: BigInt(code_id),
            msg: (0, encoding_1.toUtf8)(JSON.stringify(msg))
          })
        }
      };
    }
  }
});

// node_modules/cosmjs-types/cosmwasm/wasm/v1/tx.js
var require_tx4 = __commonJS({
  "node_modules/cosmjs-types/cosmwasm/wasm/v1/tx.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.MsgClientImpl = exports.MsgUpdateContractLabelResponse = exports.MsgUpdateContractLabel = exports.MsgStoreAndMigrateContractResponse = exports.MsgStoreAndMigrateContract = exports.MsgRemoveCodeUploadParamsAddressesResponse = exports.MsgRemoveCodeUploadParamsAddresses = exports.MsgAddCodeUploadParamsAddressesResponse = exports.MsgAddCodeUploadParamsAddresses = exports.MsgStoreAndInstantiateContractResponse = exports.MsgStoreAndInstantiateContract = exports.MsgUnpinCodesResponse = exports.MsgUnpinCodes = exports.MsgPinCodesResponse = exports.MsgPinCodes = exports.MsgSudoContractResponse = exports.MsgSudoContract = exports.MsgUpdateParamsResponse = exports.MsgUpdateParams = exports.MsgUpdateInstantiateConfigResponse = exports.MsgUpdateInstantiateConfig = exports.MsgClearAdminResponse = exports.MsgClearAdmin = exports.MsgUpdateAdminResponse = exports.MsgUpdateAdmin = exports.MsgMigrateContractResponse = exports.MsgMigrateContract = exports.MsgExecuteContractResponse = exports.MsgExecuteContract = exports.MsgInstantiateContract2Response = exports.MsgInstantiateContract2 = exports.MsgInstantiateContractResponse = exports.MsgInstantiateContract = exports.MsgStoreCodeResponse = exports.MsgStoreCode = exports.protobufPackage = void 0;
    var types_1 = require_types();
    var coin_1 = require_coin();
    var binary_1 = require_binary();
    var helpers_1 = require_helpers();
    exports.protobufPackage = "cosmwasm.wasm.v1";
    function createBaseMsgStoreCode() {
      return {
        sender: "",
        wasmByteCode: new Uint8Array(),
        instantiatePermission: void 0
      };
    }
    exports.MsgStoreCode = {
      typeUrl: "/cosmwasm.wasm.v1.MsgStoreCode",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.wasmByteCode.length !== 0) {
          writer.uint32(18).bytes(message.wasmByteCode);
        }
        if (message.instantiatePermission !== void 0) {
          types_1.AccessConfig.encode(message.instantiatePermission, writer.uint32(42).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgStoreCode();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 2:
              message.wasmByteCode = reader.bytes();
              break;
            case 5:
              message.instantiatePermission = types_1.AccessConfig.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgStoreCode();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.wasmByteCode))
          obj.wasmByteCode = (0, helpers_1.bytesFromBase64)(object.wasmByteCode);
        if ((0, helpers_1.isSet)(object.instantiatePermission))
          obj.instantiatePermission = types_1.AccessConfig.fromJSON(object.instantiatePermission);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.wasmByteCode !== void 0 && (obj.wasmByteCode = (0, helpers_1.base64FromBytes)(message.wasmByteCode !== void 0 ? message.wasmByteCode : new Uint8Array()));
        message.instantiatePermission !== void 0 && (obj.instantiatePermission = message.instantiatePermission ? types_1.AccessConfig.toJSON(message.instantiatePermission) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgStoreCode();
        message.sender = object.sender ?? "";
        message.wasmByteCode = object.wasmByteCode ?? new Uint8Array();
        if (object.instantiatePermission !== void 0 && object.instantiatePermission !== null) {
          message.instantiatePermission = types_1.AccessConfig.fromPartial(object.instantiatePermission);
        }
        return message;
      }
    };
    function createBaseMsgStoreCodeResponse() {
      return {
        codeId: BigInt(0),
        checksum: new Uint8Array()
      };
    }
    exports.MsgStoreCodeResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgStoreCodeResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeId !== BigInt(0)) {
          writer.uint32(8).uint64(message.codeId);
        }
        if (message.checksum.length !== 0) {
          writer.uint32(18).bytes(message.checksum);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgStoreCodeResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeId = reader.uint64();
              break;
            case 2:
              message.checksum = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgStoreCodeResponse();
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.checksum))
          obj.checksum = (0, helpers_1.bytesFromBase64)(object.checksum);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.checksum !== void 0 && (obj.checksum = (0, helpers_1.base64FromBytes)(message.checksum !== void 0 ? message.checksum : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgStoreCodeResponse();
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        message.checksum = object.checksum ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgInstantiateContract() {
      return {
        sender: "",
        admin: "",
        codeId: BigInt(0),
        label: "",
        msg: new Uint8Array(),
        funds: []
      };
    }
    exports.MsgInstantiateContract = {
      typeUrl: "/cosmwasm.wasm.v1.MsgInstantiateContract",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.admin !== "") {
          writer.uint32(18).string(message.admin);
        }
        if (message.codeId !== BigInt(0)) {
          writer.uint32(24).uint64(message.codeId);
        }
        if (message.label !== "") {
          writer.uint32(34).string(message.label);
        }
        if (message.msg.length !== 0) {
          writer.uint32(42).bytes(message.msg);
        }
        for (const v of message.funds) {
          coin_1.Coin.encode(v, writer.uint32(50).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgInstantiateContract();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 2:
              message.admin = reader.string();
              break;
            case 3:
              message.codeId = reader.uint64();
              break;
            case 4:
              message.label = reader.string();
              break;
            case 5:
              message.msg = reader.bytes();
              break;
            case 6:
              message.funds.push(coin_1.Coin.decode(reader, reader.uint32()));
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgInstantiateContract();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.admin))
          obj.admin = String(object.admin);
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.label))
          obj.label = String(object.label);
        if ((0, helpers_1.isSet)(object.msg))
          obj.msg = (0, helpers_1.bytesFromBase64)(object.msg);
        if (Array.isArray(object == null ? void 0 : object.funds))
          obj.funds = object.funds.map((e) => coin_1.Coin.fromJSON(e));
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.admin !== void 0 && (obj.admin = message.admin);
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.label !== void 0 && (obj.label = message.label);
        message.msg !== void 0 && (obj.msg = (0, helpers_1.base64FromBytes)(message.msg !== void 0 ? message.msg : new Uint8Array()));
        if (message.funds) {
          obj.funds = message.funds.map((e) => e ? coin_1.Coin.toJSON(e) : void 0);
        } else {
          obj.funds = [];
        }
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseMsgInstantiateContract();
        message.sender = object.sender ?? "";
        message.admin = object.admin ?? "";
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        message.label = object.label ?? "";
        message.msg = object.msg ?? new Uint8Array();
        message.funds = ((_a = object.funds) == null ? void 0 : _a.map((e) => coin_1.Coin.fromPartial(e))) || [];
        return message;
      }
    };
    function createBaseMsgInstantiateContractResponse() {
      return {
        address: "",
        data: new Uint8Array()
      };
    }
    exports.MsgInstantiateContractResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgInstantiateContractResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        if (message.data.length !== 0) {
          writer.uint32(18).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgInstantiateContractResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            case 2:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgInstantiateContractResponse();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgInstantiateContractResponse();
        message.address = object.address ?? "";
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgInstantiateContract2() {
      return {
        sender: "",
        admin: "",
        codeId: BigInt(0),
        label: "",
        msg: new Uint8Array(),
        funds: [],
        salt: new Uint8Array(),
        fixMsg: false
      };
    }
    exports.MsgInstantiateContract2 = {
      typeUrl: "/cosmwasm.wasm.v1.MsgInstantiateContract2",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.admin !== "") {
          writer.uint32(18).string(message.admin);
        }
        if (message.codeId !== BigInt(0)) {
          writer.uint32(24).uint64(message.codeId);
        }
        if (message.label !== "") {
          writer.uint32(34).string(message.label);
        }
        if (message.msg.length !== 0) {
          writer.uint32(42).bytes(message.msg);
        }
        for (const v of message.funds) {
          coin_1.Coin.encode(v, writer.uint32(50).fork()).ldelim();
        }
        if (message.salt.length !== 0) {
          writer.uint32(58).bytes(message.salt);
        }
        if (message.fixMsg === true) {
          writer.uint32(64).bool(message.fixMsg);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgInstantiateContract2();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 2:
              message.admin = reader.string();
              break;
            case 3:
              message.codeId = reader.uint64();
              break;
            case 4:
              message.label = reader.string();
              break;
            case 5:
              message.msg = reader.bytes();
              break;
            case 6:
              message.funds.push(coin_1.Coin.decode(reader, reader.uint32()));
              break;
            case 7:
              message.salt = reader.bytes();
              break;
            case 8:
              message.fixMsg = reader.bool();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgInstantiateContract2();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.admin))
          obj.admin = String(object.admin);
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.label))
          obj.label = String(object.label);
        if ((0, helpers_1.isSet)(object.msg))
          obj.msg = (0, helpers_1.bytesFromBase64)(object.msg);
        if (Array.isArray(object == null ? void 0 : object.funds))
          obj.funds = object.funds.map((e) => coin_1.Coin.fromJSON(e));
        if ((0, helpers_1.isSet)(object.salt))
          obj.salt = (0, helpers_1.bytesFromBase64)(object.salt);
        if ((0, helpers_1.isSet)(object.fixMsg))
          obj.fixMsg = Boolean(object.fixMsg);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.admin !== void 0 && (obj.admin = message.admin);
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.label !== void 0 && (obj.label = message.label);
        message.msg !== void 0 && (obj.msg = (0, helpers_1.base64FromBytes)(message.msg !== void 0 ? message.msg : new Uint8Array()));
        if (message.funds) {
          obj.funds = message.funds.map((e) => e ? coin_1.Coin.toJSON(e) : void 0);
        } else {
          obj.funds = [];
        }
        message.salt !== void 0 && (obj.salt = (0, helpers_1.base64FromBytes)(message.salt !== void 0 ? message.salt : new Uint8Array()));
        message.fixMsg !== void 0 && (obj.fixMsg = message.fixMsg);
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseMsgInstantiateContract2();
        message.sender = object.sender ?? "";
        message.admin = object.admin ?? "";
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        message.label = object.label ?? "";
        message.msg = object.msg ?? new Uint8Array();
        message.funds = ((_a = object.funds) == null ? void 0 : _a.map((e) => coin_1.Coin.fromPartial(e))) || [];
        message.salt = object.salt ?? new Uint8Array();
        message.fixMsg = object.fixMsg ?? false;
        return message;
      }
    };
    function createBaseMsgInstantiateContract2Response() {
      return {
        address: "",
        data: new Uint8Array()
      };
    }
    exports.MsgInstantiateContract2Response = {
      typeUrl: "/cosmwasm.wasm.v1.MsgInstantiateContract2Response",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        if (message.data.length !== 0) {
          writer.uint32(18).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgInstantiateContract2Response();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            case 2:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgInstantiateContract2Response();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgInstantiateContract2Response();
        message.address = object.address ?? "";
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgExecuteContract() {
      return {
        sender: "",
        contract: "",
        msg: new Uint8Array(),
        funds: []
      };
    }
    exports.MsgExecuteContract = {
      typeUrl: "/cosmwasm.wasm.v1.MsgExecuteContract",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.contract !== "") {
          writer.uint32(18).string(message.contract);
        }
        if (message.msg.length !== 0) {
          writer.uint32(26).bytes(message.msg);
        }
        for (const v of message.funds) {
          coin_1.Coin.encode(v, writer.uint32(42).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgExecuteContract();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 2:
              message.contract = reader.string();
              break;
            case 3:
              message.msg = reader.bytes();
              break;
            case 5:
              message.funds.push(coin_1.Coin.decode(reader, reader.uint32()));
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgExecuteContract();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.contract))
          obj.contract = String(object.contract);
        if ((0, helpers_1.isSet)(object.msg))
          obj.msg = (0, helpers_1.bytesFromBase64)(object.msg);
        if (Array.isArray(object == null ? void 0 : object.funds))
          obj.funds = object.funds.map((e) => coin_1.Coin.fromJSON(e));
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.contract !== void 0 && (obj.contract = message.contract);
        message.msg !== void 0 && (obj.msg = (0, helpers_1.base64FromBytes)(message.msg !== void 0 ? message.msg : new Uint8Array()));
        if (message.funds) {
          obj.funds = message.funds.map((e) => e ? coin_1.Coin.toJSON(e) : void 0);
        } else {
          obj.funds = [];
        }
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseMsgExecuteContract();
        message.sender = object.sender ?? "";
        message.contract = object.contract ?? "";
        message.msg = object.msg ?? new Uint8Array();
        message.funds = ((_a = object.funds) == null ? void 0 : _a.map((e) => coin_1.Coin.fromPartial(e))) || [];
        return message;
      }
    };
    function createBaseMsgExecuteContractResponse() {
      return {
        data: new Uint8Array()
      };
    }
    exports.MsgExecuteContractResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgExecuteContractResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.data.length !== 0) {
          writer.uint32(10).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgExecuteContractResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgExecuteContractResponse();
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgExecuteContractResponse();
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgMigrateContract() {
      return {
        sender: "",
        contract: "",
        codeId: BigInt(0),
        msg: new Uint8Array()
      };
    }
    exports.MsgMigrateContract = {
      typeUrl: "/cosmwasm.wasm.v1.MsgMigrateContract",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.contract !== "") {
          writer.uint32(18).string(message.contract);
        }
        if (message.codeId !== BigInt(0)) {
          writer.uint32(24).uint64(message.codeId);
        }
        if (message.msg.length !== 0) {
          writer.uint32(34).bytes(message.msg);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgMigrateContract();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 2:
              message.contract = reader.string();
              break;
            case 3:
              message.codeId = reader.uint64();
              break;
            case 4:
              message.msg = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgMigrateContract();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.contract))
          obj.contract = String(object.contract);
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.msg))
          obj.msg = (0, helpers_1.bytesFromBase64)(object.msg);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.contract !== void 0 && (obj.contract = message.contract);
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.msg !== void 0 && (obj.msg = (0, helpers_1.base64FromBytes)(message.msg !== void 0 ? message.msg : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgMigrateContract();
        message.sender = object.sender ?? "";
        message.contract = object.contract ?? "";
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        message.msg = object.msg ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgMigrateContractResponse() {
      return {
        data: new Uint8Array()
      };
    }
    exports.MsgMigrateContractResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgMigrateContractResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.data.length !== 0) {
          writer.uint32(10).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgMigrateContractResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgMigrateContractResponse();
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgMigrateContractResponse();
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgUpdateAdmin() {
      return {
        sender: "",
        newAdmin: "",
        contract: ""
      };
    }
    exports.MsgUpdateAdmin = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUpdateAdmin",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.newAdmin !== "") {
          writer.uint32(18).string(message.newAdmin);
        }
        if (message.contract !== "") {
          writer.uint32(26).string(message.contract);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUpdateAdmin();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 2:
              message.newAdmin = reader.string();
              break;
            case 3:
              message.contract = reader.string();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgUpdateAdmin();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.newAdmin))
          obj.newAdmin = String(object.newAdmin);
        if ((0, helpers_1.isSet)(object.contract))
          obj.contract = String(object.contract);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.newAdmin !== void 0 && (obj.newAdmin = message.newAdmin);
        message.contract !== void 0 && (obj.contract = message.contract);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgUpdateAdmin();
        message.sender = object.sender ?? "";
        message.newAdmin = object.newAdmin ?? "";
        message.contract = object.contract ?? "";
        return message;
      }
    };
    function createBaseMsgUpdateAdminResponse() {
      return {};
    }
    exports.MsgUpdateAdminResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUpdateAdminResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUpdateAdminResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgUpdateAdminResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgUpdateAdminResponse();
        return message;
      }
    };
    function createBaseMsgClearAdmin() {
      return {
        sender: "",
        contract: ""
      };
    }
    exports.MsgClearAdmin = {
      typeUrl: "/cosmwasm.wasm.v1.MsgClearAdmin",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.contract !== "") {
          writer.uint32(26).string(message.contract);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgClearAdmin();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 3:
              message.contract = reader.string();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgClearAdmin();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.contract))
          obj.contract = String(object.contract);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.contract !== void 0 && (obj.contract = message.contract);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgClearAdmin();
        message.sender = object.sender ?? "";
        message.contract = object.contract ?? "";
        return message;
      }
    };
    function createBaseMsgClearAdminResponse() {
      return {};
    }
    exports.MsgClearAdminResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgClearAdminResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgClearAdminResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgClearAdminResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgClearAdminResponse();
        return message;
      }
    };
    function createBaseMsgUpdateInstantiateConfig() {
      return {
        sender: "",
        codeId: BigInt(0),
        newInstantiatePermission: void 0
      };
    }
    exports.MsgUpdateInstantiateConfig = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUpdateInstantiateConfig",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.codeId !== BigInt(0)) {
          writer.uint32(16).uint64(message.codeId);
        }
        if (message.newInstantiatePermission !== void 0) {
          types_1.AccessConfig.encode(message.newInstantiatePermission, writer.uint32(26).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUpdateInstantiateConfig();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 2:
              message.codeId = reader.uint64();
              break;
            case 3:
              message.newInstantiatePermission = types_1.AccessConfig.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgUpdateInstantiateConfig();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.newInstantiatePermission))
          obj.newInstantiatePermission = types_1.AccessConfig.fromJSON(object.newInstantiatePermission);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.newInstantiatePermission !== void 0 && (obj.newInstantiatePermission = message.newInstantiatePermission ? types_1.AccessConfig.toJSON(message.newInstantiatePermission) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgUpdateInstantiateConfig();
        message.sender = object.sender ?? "";
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        if (object.newInstantiatePermission !== void 0 && object.newInstantiatePermission !== null) {
          message.newInstantiatePermission = types_1.AccessConfig.fromPartial(object.newInstantiatePermission);
        }
        return message;
      }
    };
    function createBaseMsgUpdateInstantiateConfigResponse() {
      return {};
    }
    exports.MsgUpdateInstantiateConfigResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUpdateInstantiateConfigResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUpdateInstantiateConfigResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgUpdateInstantiateConfigResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgUpdateInstantiateConfigResponse();
        return message;
      }
    };
    function createBaseMsgUpdateParams() {
      return {
        authority: "",
        params: types_1.Params.fromPartial({})
      };
    }
    exports.MsgUpdateParams = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUpdateParams",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.authority !== "") {
          writer.uint32(10).string(message.authority);
        }
        if (message.params !== void 0) {
          types_1.Params.encode(message.params, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUpdateParams();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.authority = reader.string();
              break;
            case 2:
              message.params = types_1.Params.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgUpdateParams();
        if ((0, helpers_1.isSet)(object.authority))
          obj.authority = String(object.authority);
        if ((0, helpers_1.isSet)(object.params))
          obj.params = types_1.Params.fromJSON(object.params);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.authority !== void 0 && (obj.authority = message.authority);
        message.params !== void 0 && (obj.params = message.params ? types_1.Params.toJSON(message.params) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgUpdateParams();
        message.authority = object.authority ?? "";
        if (object.params !== void 0 && object.params !== null) {
          message.params = types_1.Params.fromPartial(object.params);
        }
        return message;
      }
    };
    function createBaseMsgUpdateParamsResponse() {
      return {};
    }
    exports.MsgUpdateParamsResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUpdateParamsResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUpdateParamsResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgUpdateParamsResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgUpdateParamsResponse();
        return message;
      }
    };
    function createBaseMsgSudoContract() {
      return {
        authority: "",
        contract: "",
        msg: new Uint8Array()
      };
    }
    exports.MsgSudoContract = {
      typeUrl: "/cosmwasm.wasm.v1.MsgSudoContract",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.authority !== "") {
          writer.uint32(10).string(message.authority);
        }
        if (message.contract !== "") {
          writer.uint32(18).string(message.contract);
        }
        if (message.msg.length !== 0) {
          writer.uint32(26).bytes(message.msg);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgSudoContract();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.authority = reader.string();
              break;
            case 2:
              message.contract = reader.string();
              break;
            case 3:
              message.msg = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgSudoContract();
        if ((0, helpers_1.isSet)(object.authority))
          obj.authority = String(object.authority);
        if ((0, helpers_1.isSet)(object.contract))
          obj.contract = String(object.contract);
        if ((0, helpers_1.isSet)(object.msg))
          obj.msg = (0, helpers_1.bytesFromBase64)(object.msg);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.authority !== void 0 && (obj.authority = message.authority);
        message.contract !== void 0 && (obj.contract = message.contract);
        message.msg !== void 0 && (obj.msg = (0, helpers_1.base64FromBytes)(message.msg !== void 0 ? message.msg : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgSudoContract();
        message.authority = object.authority ?? "";
        message.contract = object.contract ?? "";
        message.msg = object.msg ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgSudoContractResponse() {
      return {
        data: new Uint8Array()
      };
    }
    exports.MsgSudoContractResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgSudoContractResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.data.length !== 0) {
          writer.uint32(10).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgSudoContractResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgSudoContractResponse();
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgSudoContractResponse();
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgPinCodes() {
      return {
        authority: "",
        codeIds: []
      };
    }
    exports.MsgPinCodes = {
      typeUrl: "/cosmwasm.wasm.v1.MsgPinCodes",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.authority !== "") {
          writer.uint32(10).string(message.authority);
        }
        writer.uint32(18).fork();
        for (const v of message.codeIds) {
          writer.uint64(v);
        }
        writer.ldelim();
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgPinCodes();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.authority = reader.string();
              break;
            case 2:
              if ((tag & 7) === 2) {
                const end2 = reader.uint32() + reader.pos;
                while (reader.pos < end2) {
                  message.codeIds.push(reader.uint64());
                }
              } else {
                message.codeIds.push(reader.uint64());
              }
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgPinCodes();
        if ((0, helpers_1.isSet)(object.authority))
          obj.authority = String(object.authority);
        if (Array.isArray(object == null ? void 0 : object.codeIds))
          obj.codeIds = object.codeIds.map((e) => BigInt(e.toString()));
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.authority !== void 0 && (obj.authority = message.authority);
        if (message.codeIds) {
          obj.codeIds = message.codeIds.map((e) => (e || BigInt(0)).toString());
        } else {
          obj.codeIds = [];
        }
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseMsgPinCodes();
        message.authority = object.authority ?? "";
        message.codeIds = ((_a = object.codeIds) == null ? void 0 : _a.map((e) => BigInt(e.toString()))) || [];
        return message;
      }
    };
    function createBaseMsgPinCodesResponse() {
      return {};
    }
    exports.MsgPinCodesResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgPinCodesResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgPinCodesResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgPinCodesResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgPinCodesResponse();
        return message;
      }
    };
    function createBaseMsgUnpinCodes() {
      return {
        authority: "",
        codeIds: []
      };
    }
    exports.MsgUnpinCodes = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUnpinCodes",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.authority !== "") {
          writer.uint32(10).string(message.authority);
        }
        writer.uint32(18).fork();
        for (const v of message.codeIds) {
          writer.uint64(v);
        }
        writer.ldelim();
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUnpinCodes();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.authority = reader.string();
              break;
            case 2:
              if ((tag & 7) === 2) {
                const end2 = reader.uint32() + reader.pos;
                while (reader.pos < end2) {
                  message.codeIds.push(reader.uint64());
                }
              } else {
                message.codeIds.push(reader.uint64());
              }
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgUnpinCodes();
        if ((0, helpers_1.isSet)(object.authority))
          obj.authority = String(object.authority);
        if (Array.isArray(object == null ? void 0 : object.codeIds))
          obj.codeIds = object.codeIds.map((e) => BigInt(e.toString()));
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.authority !== void 0 && (obj.authority = message.authority);
        if (message.codeIds) {
          obj.codeIds = message.codeIds.map((e) => (e || BigInt(0)).toString());
        } else {
          obj.codeIds = [];
        }
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseMsgUnpinCodes();
        message.authority = object.authority ?? "";
        message.codeIds = ((_a = object.codeIds) == null ? void 0 : _a.map((e) => BigInt(e.toString()))) || [];
        return message;
      }
    };
    function createBaseMsgUnpinCodesResponse() {
      return {};
    }
    exports.MsgUnpinCodesResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUnpinCodesResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUnpinCodesResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgUnpinCodesResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgUnpinCodesResponse();
        return message;
      }
    };
    function createBaseMsgStoreAndInstantiateContract() {
      return {
        authority: "",
        wasmByteCode: new Uint8Array(),
        instantiatePermission: void 0,
        unpinCode: false,
        admin: "",
        label: "",
        msg: new Uint8Array(),
        funds: [],
        source: "",
        builder: "",
        codeHash: new Uint8Array()
      };
    }
    exports.MsgStoreAndInstantiateContract = {
      typeUrl: "/cosmwasm.wasm.v1.MsgStoreAndInstantiateContract",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.authority !== "") {
          writer.uint32(10).string(message.authority);
        }
        if (message.wasmByteCode.length !== 0) {
          writer.uint32(26).bytes(message.wasmByteCode);
        }
        if (message.instantiatePermission !== void 0) {
          types_1.AccessConfig.encode(message.instantiatePermission, writer.uint32(34).fork()).ldelim();
        }
        if (message.unpinCode === true) {
          writer.uint32(40).bool(message.unpinCode);
        }
        if (message.admin !== "") {
          writer.uint32(50).string(message.admin);
        }
        if (message.label !== "") {
          writer.uint32(58).string(message.label);
        }
        if (message.msg.length !== 0) {
          writer.uint32(66).bytes(message.msg);
        }
        for (const v of message.funds) {
          coin_1.Coin.encode(v, writer.uint32(74).fork()).ldelim();
        }
        if (message.source !== "") {
          writer.uint32(82).string(message.source);
        }
        if (message.builder !== "") {
          writer.uint32(90).string(message.builder);
        }
        if (message.codeHash.length !== 0) {
          writer.uint32(98).bytes(message.codeHash);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgStoreAndInstantiateContract();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.authority = reader.string();
              break;
            case 3:
              message.wasmByteCode = reader.bytes();
              break;
            case 4:
              message.instantiatePermission = types_1.AccessConfig.decode(reader, reader.uint32());
              break;
            case 5:
              message.unpinCode = reader.bool();
              break;
            case 6:
              message.admin = reader.string();
              break;
            case 7:
              message.label = reader.string();
              break;
            case 8:
              message.msg = reader.bytes();
              break;
            case 9:
              message.funds.push(coin_1.Coin.decode(reader, reader.uint32()));
              break;
            case 10:
              message.source = reader.string();
              break;
            case 11:
              message.builder = reader.string();
              break;
            case 12:
              message.codeHash = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgStoreAndInstantiateContract();
        if ((0, helpers_1.isSet)(object.authority))
          obj.authority = String(object.authority);
        if ((0, helpers_1.isSet)(object.wasmByteCode))
          obj.wasmByteCode = (0, helpers_1.bytesFromBase64)(object.wasmByteCode);
        if ((0, helpers_1.isSet)(object.instantiatePermission))
          obj.instantiatePermission = types_1.AccessConfig.fromJSON(object.instantiatePermission);
        if ((0, helpers_1.isSet)(object.unpinCode))
          obj.unpinCode = Boolean(object.unpinCode);
        if ((0, helpers_1.isSet)(object.admin))
          obj.admin = String(object.admin);
        if ((0, helpers_1.isSet)(object.label))
          obj.label = String(object.label);
        if ((0, helpers_1.isSet)(object.msg))
          obj.msg = (0, helpers_1.bytesFromBase64)(object.msg);
        if (Array.isArray(object == null ? void 0 : object.funds))
          obj.funds = object.funds.map((e) => coin_1.Coin.fromJSON(e));
        if ((0, helpers_1.isSet)(object.source))
          obj.source = String(object.source);
        if ((0, helpers_1.isSet)(object.builder))
          obj.builder = String(object.builder);
        if ((0, helpers_1.isSet)(object.codeHash))
          obj.codeHash = (0, helpers_1.bytesFromBase64)(object.codeHash);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.authority !== void 0 && (obj.authority = message.authority);
        message.wasmByteCode !== void 0 && (obj.wasmByteCode = (0, helpers_1.base64FromBytes)(message.wasmByteCode !== void 0 ? message.wasmByteCode : new Uint8Array()));
        message.instantiatePermission !== void 0 && (obj.instantiatePermission = message.instantiatePermission ? types_1.AccessConfig.toJSON(message.instantiatePermission) : void 0);
        message.unpinCode !== void 0 && (obj.unpinCode = message.unpinCode);
        message.admin !== void 0 && (obj.admin = message.admin);
        message.label !== void 0 && (obj.label = message.label);
        message.msg !== void 0 && (obj.msg = (0, helpers_1.base64FromBytes)(message.msg !== void 0 ? message.msg : new Uint8Array()));
        if (message.funds) {
          obj.funds = message.funds.map((e) => e ? coin_1.Coin.toJSON(e) : void 0);
        } else {
          obj.funds = [];
        }
        message.source !== void 0 && (obj.source = message.source);
        message.builder !== void 0 && (obj.builder = message.builder);
        message.codeHash !== void 0 && (obj.codeHash = (0, helpers_1.base64FromBytes)(message.codeHash !== void 0 ? message.codeHash : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseMsgStoreAndInstantiateContract();
        message.authority = object.authority ?? "";
        message.wasmByteCode = object.wasmByteCode ?? new Uint8Array();
        if (object.instantiatePermission !== void 0 && object.instantiatePermission !== null) {
          message.instantiatePermission = types_1.AccessConfig.fromPartial(object.instantiatePermission);
        }
        message.unpinCode = object.unpinCode ?? false;
        message.admin = object.admin ?? "";
        message.label = object.label ?? "";
        message.msg = object.msg ?? new Uint8Array();
        message.funds = ((_a = object.funds) == null ? void 0 : _a.map((e) => coin_1.Coin.fromPartial(e))) || [];
        message.source = object.source ?? "";
        message.builder = object.builder ?? "";
        message.codeHash = object.codeHash ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgStoreAndInstantiateContractResponse() {
      return {
        address: "",
        data: new Uint8Array()
      };
    }
    exports.MsgStoreAndInstantiateContractResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgStoreAndInstantiateContractResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        if (message.data.length !== 0) {
          writer.uint32(18).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgStoreAndInstantiateContractResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            case 2:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgStoreAndInstantiateContractResponse();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgStoreAndInstantiateContractResponse();
        message.address = object.address ?? "";
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgAddCodeUploadParamsAddresses() {
      return {
        authority: "",
        addresses: []
      };
    }
    exports.MsgAddCodeUploadParamsAddresses = {
      typeUrl: "/cosmwasm.wasm.v1.MsgAddCodeUploadParamsAddresses",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.authority !== "") {
          writer.uint32(10).string(message.authority);
        }
        for (const v of message.addresses) {
          writer.uint32(18).string(v);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgAddCodeUploadParamsAddresses();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.authority = reader.string();
              break;
            case 2:
              message.addresses.push(reader.string());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgAddCodeUploadParamsAddresses();
        if ((0, helpers_1.isSet)(object.authority))
          obj.authority = String(object.authority);
        if (Array.isArray(object == null ? void 0 : object.addresses))
          obj.addresses = object.addresses.map((e) => String(e));
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.authority !== void 0 && (obj.authority = message.authority);
        if (message.addresses) {
          obj.addresses = message.addresses.map((e) => e);
        } else {
          obj.addresses = [];
        }
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseMsgAddCodeUploadParamsAddresses();
        message.authority = object.authority ?? "";
        message.addresses = ((_a = object.addresses) == null ? void 0 : _a.map((e) => e)) || [];
        return message;
      }
    };
    function createBaseMsgAddCodeUploadParamsAddressesResponse() {
      return {};
    }
    exports.MsgAddCodeUploadParamsAddressesResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgAddCodeUploadParamsAddressesResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgAddCodeUploadParamsAddressesResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgAddCodeUploadParamsAddressesResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgAddCodeUploadParamsAddressesResponse();
        return message;
      }
    };
    function createBaseMsgRemoveCodeUploadParamsAddresses() {
      return {
        authority: "",
        addresses: []
      };
    }
    exports.MsgRemoveCodeUploadParamsAddresses = {
      typeUrl: "/cosmwasm.wasm.v1.MsgRemoveCodeUploadParamsAddresses",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.authority !== "") {
          writer.uint32(10).string(message.authority);
        }
        for (const v of message.addresses) {
          writer.uint32(18).string(v);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgRemoveCodeUploadParamsAddresses();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.authority = reader.string();
              break;
            case 2:
              message.addresses.push(reader.string());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgRemoveCodeUploadParamsAddresses();
        if ((0, helpers_1.isSet)(object.authority))
          obj.authority = String(object.authority);
        if (Array.isArray(object == null ? void 0 : object.addresses))
          obj.addresses = object.addresses.map((e) => String(e));
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.authority !== void 0 && (obj.authority = message.authority);
        if (message.addresses) {
          obj.addresses = message.addresses.map((e) => e);
        } else {
          obj.addresses = [];
        }
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseMsgRemoveCodeUploadParamsAddresses();
        message.authority = object.authority ?? "";
        message.addresses = ((_a = object.addresses) == null ? void 0 : _a.map((e) => e)) || [];
        return message;
      }
    };
    function createBaseMsgRemoveCodeUploadParamsAddressesResponse() {
      return {};
    }
    exports.MsgRemoveCodeUploadParamsAddressesResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgRemoveCodeUploadParamsAddressesResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgRemoveCodeUploadParamsAddressesResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgRemoveCodeUploadParamsAddressesResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgRemoveCodeUploadParamsAddressesResponse();
        return message;
      }
    };
    function createBaseMsgStoreAndMigrateContract() {
      return {
        authority: "",
        wasmByteCode: new Uint8Array(),
        instantiatePermission: void 0,
        contract: "",
        msg: new Uint8Array()
      };
    }
    exports.MsgStoreAndMigrateContract = {
      typeUrl: "/cosmwasm.wasm.v1.MsgStoreAndMigrateContract",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.authority !== "") {
          writer.uint32(10).string(message.authority);
        }
        if (message.wasmByteCode.length !== 0) {
          writer.uint32(18).bytes(message.wasmByteCode);
        }
        if (message.instantiatePermission !== void 0) {
          types_1.AccessConfig.encode(message.instantiatePermission, writer.uint32(26).fork()).ldelim();
        }
        if (message.contract !== "") {
          writer.uint32(34).string(message.contract);
        }
        if (message.msg.length !== 0) {
          writer.uint32(42).bytes(message.msg);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgStoreAndMigrateContract();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.authority = reader.string();
              break;
            case 2:
              message.wasmByteCode = reader.bytes();
              break;
            case 3:
              message.instantiatePermission = types_1.AccessConfig.decode(reader, reader.uint32());
              break;
            case 4:
              message.contract = reader.string();
              break;
            case 5:
              message.msg = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgStoreAndMigrateContract();
        if ((0, helpers_1.isSet)(object.authority))
          obj.authority = String(object.authority);
        if ((0, helpers_1.isSet)(object.wasmByteCode))
          obj.wasmByteCode = (0, helpers_1.bytesFromBase64)(object.wasmByteCode);
        if ((0, helpers_1.isSet)(object.instantiatePermission))
          obj.instantiatePermission = types_1.AccessConfig.fromJSON(object.instantiatePermission);
        if ((0, helpers_1.isSet)(object.contract))
          obj.contract = String(object.contract);
        if ((0, helpers_1.isSet)(object.msg))
          obj.msg = (0, helpers_1.bytesFromBase64)(object.msg);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.authority !== void 0 && (obj.authority = message.authority);
        message.wasmByteCode !== void 0 && (obj.wasmByteCode = (0, helpers_1.base64FromBytes)(message.wasmByteCode !== void 0 ? message.wasmByteCode : new Uint8Array()));
        message.instantiatePermission !== void 0 && (obj.instantiatePermission = message.instantiatePermission ? types_1.AccessConfig.toJSON(message.instantiatePermission) : void 0);
        message.contract !== void 0 && (obj.contract = message.contract);
        message.msg !== void 0 && (obj.msg = (0, helpers_1.base64FromBytes)(message.msg !== void 0 ? message.msg : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgStoreAndMigrateContract();
        message.authority = object.authority ?? "";
        message.wasmByteCode = object.wasmByteCode ?? new Uint8Array();
        if (object.instantiatePermission !== void 0 && object.instantiatePermission !== null) {
          message.instantiatePermission = types_1.AccessConfig.fromPartial(object.instantiatePermission);
        }
        message.contract = object.contract ?? "";
        message.msg = object.msg ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgStoreAndMigrateContractResponse() {
      return {
        codeId: BigInt(0),
        checksum: new Uint8Array(),
        data: new Uint8Array()
      };
    }
    exports.MsgStoreAndMigrateContractResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgStoreAndMigrateContractResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeId !== BigInt(0)) {
          writer.uint32(8).uint64(message.codeId);
        }
        if (message.checksum.length !== 0) {
          writer.uint32(18).bytes(message.checksum);
        }
        if (message.data.length !== 0) {
          writer.uint32(26).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgStoreAndMigrateContractResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeId = reader.uint64();
              break;
            case 2:
              message.checksum = reader.bytes();
              break;
            case 3:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgStoreAndMigrateContractResponse();
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.checksum))
          obj.checksum = (0, helpers_1.bytesFromBase64)(object.checksum);
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.checksum !== void 0 && (obj.checksum = (0, helpers_1.base64FromBytes)(message.checksum !== void 0 ? message.checksum : new Uint8Array()));
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgStoreAndMigrateContractResponse();
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        message.checksum = object.checksum ?? new Uint8Array();
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseMsgUpdateContractLabel() {
      return {
        sender: "",
        newLabel: "",
        contract: ""
      };
    }
    exports.MsgUpdateContractLabel = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUpdateContractLabel",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.sender !== "") {
          writer.uint32(10).string(message.sender);
        }
        if (message.newLabel !== "") {
          writer.uint32(18).string(message.newLabel);
        }
        if (message.contract !== "") {
          writer.uint32(26).string(message.contract);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUpdateContractLabel();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.sender = reader.string();
              break;
            case 2:
              message.newLabel = reader.string();
              break;
            case 3:
              message.contract = reader.string();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseMsgUpdateContractLabel();
        if ((0, helpers_1.isSet)(object.sender))
          obj.sender = String(object.sender);
        if ((0, helpers_1.isSet)(object.newLabel))
          obj.newLabel = String(object.newLabel);
        if ((0, helpers_1.isSet)(object.contract))
          obj.contract = String(object.contract);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.sender !== void 0 && (obj.sender = message.sender);
        message.newLabel !== void 0 && (obj.newLabel = message.newLabel);
        message.contract !== void 0 && (obj.contract = message.contract);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseMsgUpdateContractLabel();
        message.sender = object.sender ?? "";
        message.newLabel = object.newLabel ?? "";
        message.contract = object.contract ?? "";
        return message;
      }
    };
    function createBaseMsgUpdateContractLabelResponse() {
      return {};
    }
    exports.MsgUpdateContractLabelResponse = {
      typeUrl: "/cosmwasm.wasm.v1.MsgUpdateContractLabelResponse",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseMsgUpdateContractLabelResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseMsgUpdateContractLabelResponse();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseMsgUpdateContractLabelResponse();
        return message;
      }
    };
    var MsgClientImpl = class {
      constructor(rpc) {
        this.rpc = rpc;
        this.StoreCode = this.StoreCode.bind(this);
        this.InstantiateContract = this.InstantiateContract.bind(this);
        this.InstantiateContract2 = this.InstantiateContract2.bind(this);
        this.ExecuteContract = this.ExecuteContract.bind(this);
        this.MigrateContract = this.MigrateContract.bind(this);
        this.UpdateAdmin = this.UpdateAdmin.bind(this);
        this.ClearAdmin = this.ClearAdmin.bind(this);
        this.UpdateInstantiateConfig = this.UpdateInstantiateConfig.bind(this);
        this.UpdateParams = this.UpdateParams.bind(this);
        this.SudoContract = this.SudoContract.bind(this);
        this.PinCodes = this.PinCodes.bind(this);
        this.UnpinCodes = this.UnpinCodes.bind(this);
        this.StoreAndInstantiateContract = this.StoreAndInstantiateContract.bind(this);
        this.RemoveCodeUploadParamsAddresses = this.RemoveCodeUploadParamsAddresses.bind(this);
        this.AddCodeUploadParamsAddresses = this.AddCodeUploadParamsAddresses.bind(this);
        this.StoreAndMigrateContract = this.StoreAndMigrateContract.bind(this);
        this.UpdateContractLabel = this.UpdateContractLabel.bind(this);
      }
      StoreCode(request) {
        const data = exports.MsgStoreCode.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "StoreCode", data);
        return promise.then((data2) => exports.MsgStoreCodeResponse.decode(new binary_1.BinaryReader(data2)));
      }
      InstantiateContract(request) {
        const data = exports.MsgInstantiateContract.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "InstantiateContract", data);
        return promise.then((data2) => exports.MsgInstantiateContractResponse.decode(new binary_1.BinaryReader(data2)));
      }
      InstantiateContract2(request) {
        const data = exports.MsgInstantiateContract2.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "InstantiateContract2", data);
        return promise.then((data2) => exports.MsgInstantiateContract2Response.decode(new binary_1.BinaryReader(data2)));
      }
      ExecuteContract(request) {
        const data = exports.MsgExecuteContract.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "ExecuteContract", data);
        return promise.then((data2) => exports.MsgExecuteContractResponse.decode(new binary_1.BinaryReader(data2)));
      }
      MigrateContract(request) {
        const data = exports.MsgMigrateContract.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "MigrateContract", data);
        return promise.then((data2) => exports.MsgMigrateContractResponse.decode(new binary_1.BinaryReader(data2)));
      }
      UpdateAdmin(request) {
        const data = exports.MsgUpdateAdmin.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "UpdateAdmin", data);
        return promise.then((data2) => exports.MsgUpdateAdminResponse.decode(new binary_1.BinaryReader(data2)));
      }
      ClearAdmin(request) {
        const data = exports.MsgClearAdmin.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "ClearAdmin", data);
        return promise.then((data2) => exports.MsgClearAdminResponse.decode(new binary_1.BinaryReader(data2)));
      }
      UpdateInstantiateConfig(request) {
        const data = exports.MsgUpdateInstantiateConfig.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "UpdateInstantiateConfig", data);
        return promise.then((data2) => exports.MsgUpdateInstantiateConfigResponse.decode(new binary_1.BinaryReader(data2)));
      }
      UpdateParams(request) {
        const data = exports.MsgUpdateParams.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "UpdateParams", data);
        return promise.then((data2) => exports.MsgUpdateParamsResponse.decode(new binary_1.BinaryReader(data2)));
      }
      SudoContract(request) {
        const data = exports.MsgSudoContract.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "SudoContract", data);
        return promise.then((data2) => exports.MsgSudoContractResponse.decode(new binary_1.BinaryReader(data2)));
      }
      PinCodes(request) {
        const data = exports.MsgPinCodes.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "PinCodes", data);
        return promise.then((data2) => exports.MsgPinCodesResponse.decode(new binary_1.BinaryReader(data2)));
      }
      UnpinCodes(request) {
        const data = exports.MsgUnpinCodes.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "UnpinCodes", data);
        return promise.then((data2) => exports.MsgUnpinCodesResponse.decode(new binary_1.BinaryReader(data2)));
      }
      StoreAndInstantiateContract(request) {
        const data = exports.MsgStoreAndInstantiateContract.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "StoreAndInstantiateContract", data);
        return promise.then((data2) => exports.MsgStoreAndInstantiateContractResponse.decode(new binary_1.BinaryReader(data2)));
      }
      RemoveCodeUploadParamsAddresses(request) {
        const data = exports.MsgRemoveCodeUploadParamsAddresses.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "RemoveCodeUploadParamsAddresses", data);
        return promise.then((data2) => exports.MsgRemoveCodeUploadParamsAddressesResponse.decode(new binary_1.BinaryReader(data2)));
      }
      AddCodeUploadParamsAddresses(request) {
        const data = exports.MsgAddCodeUploadParamsAddresses.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "AddCodeUploadParamsAddresses", data);
        return promise.then((data2) => exports.MsgAddCodeUploadParamsAddressesResponse.decode(new binary_1.BinaryReader(data2)));
      }
      StoreAndMigrateContract(request) {
        const data = exports.MsgStoreAndMigrateContract.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "StoreAndMigrateContract", data);
        return promise.then((data2) => exports.MsgStoreAndMigrateContractResponse.decode(new binary_1.BinaryReader(data2)));
      }
      UpdateContractLabel(request) {
        const data = exports.MsgUpdateContractLabel.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Msg", "UpdateContractLabel", data);
        return promise.then((data2) => exports.MsgUpdateContractLabelResponse.decode(new binary_1.BinaryReader(data2)));
      }
    };
    exports.MsgClientImpl = MsgClientImpl;
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/modules/wasm/messages.js
var require_messages = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/modules/wasm/messages.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.wasmTypes = void 0;
    exports.isMsgStoreCodeEncodeObject = isMsgStoreCodeEncodeObject;
    exports.isMsgInstantiateContractEncodeObject = isMsgInstantiateContractEncodeObject;
    exports.isMsgInstantiateContract2EncodeObject = isMsgInstantiateContract2EncodeObject;
    exports.isMsgUpdateAdminEncodeObject = isMsgUpdateAdminEncodeObject;
    exports.isMsgClearAdminEncodeObject = isMsgClearAdminEncodeObject;
    exports.isMsgMigrateEncodeObject = isMsgMigrateEncodeObject;
    exports.isMsgExecuteEncodeObject = isMsgExecuteEncodeObject;
    var tx_1 = require_tx4();
    exports.wasmTypes = [
      ["/cosmwasm.wasm.v1.MsgClearAdmin", tx_1.MsgClearAdmin],
      ["/cosmwasm.wasm.v1.MsgExecuteContract", tx_1.MsgExecuteContract],
      ["/cosmwasm.wasm.v1.MsgMigrateContract", tx_1.MsgMigrateContract],
      ["/cosmwasm.wasm.v1.MsgStoreCode", tx_1.MsgStoreCode],
      ["/cosmwasm.wasm.v1.MsgInstantiateContract", tx_1.MsgInstantiateContract],
      ["/cosmwasm.wasm.v1.MsgInstantiateContract2", tx_1.MsgInstantiateContract2],
      ["/cosmwasm.wasm.v1.MsgUpdateAdmin", tx_1.MsgUpdateAdmin]
    ];
    function isMsgStoreCodeEncodeObject(object) {
      return object.typeUrl === "/cosmwasm.wasm.v1.MsgStoreCode";
    }
    function isMsgInstantiateContractEncodeObject(object) {
      return object.typeUrl === "/cosmwasm.wasm.v1.MsgInstantiateContract";
    }
    function isMsgInstantiateContract2EncodeObject(object) {
      return object.typeUrl === "/cosmwasm.wasm.v1.MsgInstantiateContract2";
    }
    function isMsgUpdateAdminEncodeObject(object) {
      return object.typeUrl === "/cosmwasm.wasm.v1.MsgUpdateAdmin";
    }
    function isMsgClearAdminEncodeObject(object) {
      return object.typeUrl === "/cosmwasm.wasm.v1.MsgClearAdmin";
    }
    function isMsgMigrateEncodeObject(object) {
      return object.typeUrl === "/cosmwasm.wasm.v1.MsgMigrateContract";
    }
    function isMsgExecuteEncodeObject(object) {
      return object.typeUrl === "/cosmwasm.wasm.v1.MsgExecuteContract";
    }
  }
});

// node_modules/cosmjs-types/cosmwasm/wasm/v1/query.js
var require_query = __commonJS({
  "node_modules/cosmjs-types/cosmwasm/wasm/v1/query.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.QueryClientImpl = exports.QueryBuildAddressResponse = exports.QueryBuildAddressRequest = exports.QueryWasmLimitsConfigResponse = exports.QueryWasmLimitsConfigRequest = exports.QueryContractsByCreatorResponse = exports.QueryContractsByCreatorRequest = exports.QueryParamsResponse = exports.QueryParamsRequest = exports.QueryPinnedCodesResponse = exports.QueryPinnedCodesRequest = exports.QueryCodesResponse = exports.QueryCodesRequest = exports.QueryCodeResponse = exports.CodeInfoResponse = exports.QueryCodeInfoResponse = exports.QueryCodeInfoRequest = exports.QueryCodeRequest = exports.QuerySmartContractStateResponse = exports.QuerySmartContractStateRequest = exports.QueryRawContractStateResponse = exports.QueryRawContractStateRequest = exports.QueryAllContractStateResponse = exports.QueryAllContractStateRequest = exports.QueryContractsByCodeResponse = exports.QueryContractsByCodeRequest = exports.QueryContractHistoryResponse = exports.QueryContractHistoryRequest = exports.QueryContractInfoResponse = exports.QueryContractInfoRequest = exports.protobufPackage = void 0;
    var pagination_1 = require_pagination();
    var types_1 = require_types();
    var binary_1 = require_binary();
    var helpers_1 = require_helpers();
    exports.protobufPackage = "cosmwasm.wasm.v1";
    function createBaseQueryContractInfoRequest() {
      return {
        address: ""
      };
    }
    exports.QueryContractInfoRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryContractInfoRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryContractInfoRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryContractInfoRequest();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryContractInfoRequest();
        message.address = object.address ?? "";
        return message;
      }
    };
    function createBaseQueryContractInfoResponse() {
      return {
        address: "",
        contractInfo: types_1.ContractInfo.fromPartial({})
      };
    }
    exports.QueryContractInfoResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryContractInfoResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        if (message.contractInfo !== void 0) {
          types_1.ContractInfo.encode(message.contractInfo, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryContractInfoResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            case 2:
              message.contractInfo = types_1.ContractInfo.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryContractInfoResponse();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        if ((0, helpers_1.isSet)(object.contractInfo))
          obj.contractInfo = types_1.ContractInfo.fromJSON(object.contractInfo);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        message.contractInfo !== void 0 && (obj.contractInfo = message.contractInfo ? types_1.ContractInfo.toJSON(message.contractInfo) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryContractInfoResponse();
        message.address = object.address ?? "";
        if (object.contractInfo !== void 0 && object.contractInfo !== null) {
          message.contractInfo = types_1.ContractInfo.fromPartial(object.contractInfo);
        }
        return message;
      }
    };
    function createBaseQueryContractHistoryRequest() {
      return {
        address: "",
        pagination: void 0
      };
    }
    exports.QueryContractHistoryRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryContractHistoryRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        if (message.pagination !== void 0) {
          pagination_1.PageRequest.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryContractHistoryRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            case 2:
              message.pagination = pagination_1.PageRequest.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryContractHistoryRequest();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageRequest.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageRequest.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryContractHistoryRequest();
        message.address = object.address ?? "";
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageRequest.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryContractHistoryResponse() {
      return {
        entries: [],
        pagination: void 0
      };
    }
    exports.QueryContractHistoryResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryContractHistoryResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        for (const v of message.entries) {
          types_1.ContractCodeHistoryEntry.encode(v, writer.uint32(10).fork()).ldelim();
        }
        if (message.pagination !== void 0) {
          pagination_1.PageResponse.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryContractHistoryResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.entries.push(types_1.ContractCodeHistoryEntry.decode(reader, reader.uint32()));
              break;
            case 2:
              message.pagination = pagination_1.PageResponse.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryContractHistoryResponse();
        if (Array.isArray(object == null ? void 0 : object.entries))
          obj.entries = object.entries.map((e) => types_1.ContractCodeHistoryEntry.fromJSON(e));
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageResponse.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        if (message.entries) {
          obj.entries = message.entries.map((e) => e ? types_1.ContractCodeHistoryEntry.toJSON(e) : void 0);
        } else {
          obj.entries = [];
        }
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageResponse.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseQueryContractHistoryResponse();
        message.entries = ((_a = object.entries) == null ? void 0 : _a.map((e) => types_1.ContractCodeHistoryEntry.fromPartial(e))) || [];
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageResponse.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryContractsByCodeRequest() {
      return {
        codeId: BigInt(0),
        pagination: void 0
      };
    }
    exports.QueryContractsByCodeRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryContractsByCodeRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeId !== BigInt(0)) {
          writer.uint32(8).uint64(message.codeId);
        }
        if (message.pagination !== void 0) {
          pagination_1.PageRequest.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryContractsByCodeRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeId = reader.uint64();
              break;
            case 2:
              message.pagination = pagination_1.PageRequest.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryContractsByCodeRequest();
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageRequest.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageRequest.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryContractsByCodeRequest();
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageRequest.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryContractsByCodeResponse() {
      return {
        contracts: [],
        pagination: void 0
      };
    }
    exports.QueryContractsByCodeResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryContractsByCodeResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        for (const v of message.contracts) {
          writer.uint32(10).string(v);
        }
        if (message.pagination !== void 0) {
          pagination_1.PageResponse.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryContractsByCodeResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.contracts.push(reader.string());
              break;
            case 2:
              message.pagination = pagination_1.PageResponse.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryContractsByCodeResponse();
        if (Array.isArray(object == null ? void 0 : object.contracts))
          obj.contracts = object.contracts.map((e) => String(e));
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageResponse.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        if (message.contracts) {
          obj.contracts = message.contracts.map((e) => e);
        } else {
          obj.contracts = [];
        }
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageResponse.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseQueryContractsByCodeResponse();
        message.contracts = ((_a = object.contracts) == null ? void 0 : _a.map((e) => e)) || [];
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageResponse.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryAllContractStateRequest() {
      return {
        address: "",
        pagination: void 0
      };
    }
    exports.QueryAllContractStateRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryAllContractStateRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        if (message.pagination !== void 0) {
          pagination_1.PageRequest.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryAllContractStateRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            case 2:
              message.pagination = pagination_1.PageRequest.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryAllContractStateRequest();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageRequest.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageRequest.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryAllContractStateRequest();
        message.address = object.address ?? "";
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageRequest.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryAllContractStateResponse() {
      return {
        models: [],
        pagination: void 0
      };
    }
    exports.QueryAllContractStateResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryAllContractStateResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        for (const v of message.models) {
          types_1.Model.encode(v, writer.uint32(10).fork()).ldelim();
        }
        if (message.pagination !== void 0) {
          pagination_1.PageResponse.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryAllContractStateResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.models.push(types_1.Model.decode(reader, reader.uint32()));
              break;
            case 2:
              message.pagination = pagination_1.PageResponse.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryAllContractStateResponse();
        if (Array.isArray(object == null ? void 0 : object.models))
          obj.models = object.models.map((e) => types_1.Model.fromJSON(e));
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageResponse.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        if (message.models) {
          obj.models = message.models.map((e) => e ? types_1.Model.toJSON(e) : void 0);
        } else {
          obj.models = [];
        }
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageResponse.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseQueryAllContractStateResponse();
        message.models = ((_a = object.models) == null ? void 0 : _a.map((e) => types_1.Model.fromPartial(e))) || [];
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageResponse.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryRawContractStateRequest() {
      return {
        address: "",
        queryData: new Uint8Array()
      };
    }
    exports.QueryRawContractStateRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryRawContractStateRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        if (message.queryData.length !== 0) {
          writer.uint32(18).bytes(message.queryData);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryRawContractStateRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            case 2:
              message.queryData = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryRawContractStateRequest();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        if ((0, helpers_1.isSet)(object.queryData))
          obj.queryData = (0, helpers_1.bytesFromBase64)(object.queryData);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        message.queryData !== void 0 && (obj.queryData = (0, helpers_1.base64FromBytes)(message.queryData !== void 0 ? message.queryData : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryRawContractStateRequest();
        message.address = object.address ?? "";
        message.queryData = object.queryData ?? new Uint8Array();
        return message;
      }
    };
    function createBaseQueryRawContractStateResponse() {
      return {
        data: new Uint8Array()
      };
    }
    exports.QueryRawContractStateResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryRawContractStateResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.data.length !== 0) {
          writer.uint32(10).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryRawContractStateResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryRawContractStateResponse();
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryRawContractStateResponse();
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseQuerySmartContractStateRequest() {
      return {
        address: "",
        queryData: new Uint8Array()
      };
    }
    exports.QuerySmartContractStateRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QuerySmartContractStateRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        if (message.queryData.length !== 0) {
          writer.uint32(18).bytes(message.queryData);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQuerySmartContractStateRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            case 2:
              message.queryData = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQuerySmartContractStateRequest();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        if ((0, helpers_1.isSet)(object.queryData))
          obj.queryData = (0, helpers_1.bytesFromBase64)(object.queryData);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        message.queryData !== void 0 && (obj.queryData = (0, helpers_1.base64FromBytes)(message.queryData !== void 0 ? message.queryData : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQuerySmartContractStateRequest();
        message.address = object.address ?? "";
        message.queryData = object.queryData ?? new Uint8Array();
        return message;
      }
    };
    function createBaseQuerySmartContractStateResponse() {
      return {
        data: new Uint8Array()
      };
    }
    exports.QuerySmartContractStateResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QuerySmartContractStateResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.data.length !== 0) {
          writer.uint32(10).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQuerySmartContractStateResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQuerySmartContractStateResponse();
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQuerySmartContractStateResponse();
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseQueryCodeRequest() {
      return {
        codeId: BigInt(0)
      };
    }
    exports.QueryCodeRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryCodeRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeId !== BigInt(0)) {
          writer.uint32(8).uint64(message.codeId);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryCodeRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeId = reader.uint64();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryCodeRequest();
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryCodeRequest();
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        return message;
      }
    };
    function createBaseQueryCodeInfoRequest() {
      return {
        codeId: BigInt(0)
      };
    }
    exports.QueryCodeInfoRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryCodeInfoRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeId !== BigInt(0)) {
          writer.uint32(8).uint64(message.codeId);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryCodeInfoRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeId = reader.uint64();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryCodeInfoRequest();
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryCodeInfoRequest();
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        return message;
      }
    };
    function createBaseQueryCodeInfoResponse() {
      return {
        codeId: BigInt(0),
        creator: "",
        checksum: new Uint8Array(),
        instantiatePermission: types_1.AccessConfig.fromPartial({})
      };
    }
    exports.QueryCodeInfoResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryCodeInfoResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeId !== BigInt(0)) {
          writer.uint32(8).uint64(message.codeId);
        }
        if (message.creator !== "") {
          writer.uint32(18).string(message.creator);
        }
        if (message.checksum.length !== 0) {
          writer.uint32(26).bytes(message.checksum);
        }
        if (message.instantiatePermission !== void 0) {
          types_1.AccessConfig.encode(message.instantiatePermission, writer.uint32(34).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryCodeInfoResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeId = reader.uint64();
              break;
            case 2:
              message.creator = reader.string();
              break;
            case 3:
              message.checksum = reader.bytes();
              break;
            case 4:
              message.instantiatePermission = types_1.AccessConfig.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryCodeInfoResponse();
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.creator))
          obj.creator = String(object.creator);
        if ((0, helpers_1.isSet)(object.checksum))
          obj.checksum = (0, helpers_1.bytesFromBase64)(object.checksum);
        if ((0, helpers_1.isSet)(object.instantiatePermission))
          obj.instantiatePermission = types_1.AccessConfig.fromJSON(object.instantiatePermission);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.creator !== void 0 && (obj.creator = message.creator);
        message.checksum !== void 0 && (obj.checksum = (0, helpers_1.base64FromBytes)(message.checksum !== void 0 ? message.checksum : new Uint8Array()));
        message.instantiatePermission !== void 0 && (obj.instantiatePermission = message.instantiatePermission ? types_1.AccessConfig.toJSON(message.instantiatePermission) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryCodeInfoResponse();
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        message.creator = object.creator ?? "";
        message.checksum = object.checksum ?? new Uint8Array();
        if (object.instantiatePermission !== void 0 && object.instantiatePermission !== null) {
          message.instantiatePermission = types_1.AccessConfig.fromPartial(object.instantiatePermission);
        }
        return message;
      }
    };
    function createBaseCodeInfoResponse() {
      return {
        codeId: BigInt(0),
        creator: "",
        dataHash: new Uint8Array(),
        instantiatePermission: types_1.AccessConfig.fromPartial({})
      };
    }
    exports.CodeInfoResponse = {
      typeUrl: "/cosmwasm.wasm.v1.CodeInfoResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeId !== BigInt(0)) {
          writer.uint32(8).uint64(message.codeId);
        }
        if (message.creator !== "") {
          writer.uint32(18).string(message.creator);
        }
        if (message.dataHash.length !== 0) {
          writer.uint32(26).bytes(message.dataHash);
        }
        if (message.instantiatePermission !== void 0) {
          types_1.AccessConfig.encode(message.instantiatePermission, writer.uint32(50).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseCodeInfoResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeId = reader.uint64();
              break;
            case 2:
              message.creator = reader.string();
              break;
            case 3:
              message.dataHash = reader.bytes();
              break;
            case 6:
              message.instantiatePermission = types_1.AccessConfig.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseCodeInfoResponse();
        if ((0, helpers_1.isSet)(object.codeId))
          obj.codeId = BigInt(object.codeId.toString());
        if ((0, helpers_1.isSet)(object.creator))
          obj.creator = String(object.creator);
        if ((0, helpers_1.isSet)(object.dataHash))
          obj.dataHash = (0, helpers_1.bytesFromBase64)(object.dataHash);
        if ((0, helpers_1.isSet)(object.instantiatePermission))
          obj.instantiatePermission = types_1.AccessConfig.fromJSON(object.instantiatePermission);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeId !== void 0 && (obj.codeId = (message.codeId || BigInt(0)).toString());
        message.creator !== void 0 && (obj.creator = message.creator);
        message.dataHash !== void 0 && (obj.dataHash = (0, helpers_1.base64FromBytes)(message.dataHash !== void 0 ? message.dataHash : new Uint8Array()));
        message.instantiatePermission !== void 0 && (obj.instantiatePermission = message.instantiatePermission ? types_1.AccessConfig.toJSON(message.instantiatePermission) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseCodeInfoResponse();
        if (object.codeId !== void 0 && object.codeId !== null) {
          message.codeId = BigInt(object.codeId.toString());
        }
        message.creator = object.creator ?? "";
        message.dataHash = object.dataHash ?? new Uint8Array();
        if (object.instantiatePermission !== void 0 && object.instantiatePermission !== null) {
          message.instantiatePermission = types_1.AccessConfig.fromPartial(object.instantiatePermission);
        }
        return message;
      }
    };
    function createBaseQueryCodeResponse() {
      return {
        codeInfo: void 0,
        data: new Uint8Array()
      };
    }
    exports.QueryCodeResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryCodeResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeInfo !== void 0) {
          exports.CodeInfoResponse.encode(message.codeInfo, writer.uint32(10).fork()).ldelim();
        }
        if (message.data.length !== 0) {
          writer.uint32(18).bytes(message.data);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryCodeResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeInfo = exports.CodeInfoResponse.decode(reader, reader.uint32());
              break;
            case 2:
              message.data = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryCodeResponse();
        if ((0, helpers_1.isSet)(object.codeInfo))
          obj.codeInfo = exports.CodeInfoResponse.fromJSON(object.codeInfo);
        if ((0, helpers_1.isSet)(object.data))
          obj.data = (0, helpers_1.bytesFromBase64)(object.data);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeInfo !== void 0 && (obj.codeInfo = message.codeInfo ? exports.CodeInfoResponse.toJSON(message.codeInfo) : void 0);
        message.data !== void 0 && (obj.data = (0, helpers_1.base64FromBytes)(message.data !== void 0 ? message.data : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryCodeResponse();
        if (object.codeInfo !== void 0 && object.codeInfo !== null) {
          message.codeInfo = exports.CodeInfoResponse.fromPartial(object.codeInfo);
        }
        message.data = object.data ?? new Uint8Array();
        return message;
      }
    };
    function createBaseQueryCodesRequest() {
      return {
        pagination: void 0
      };
    }
    exports.QueryCodesRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryCodesRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.pagination !== void 0) {
          pagination_1.PageRequest.encode(message.pagination, writer.uint32(10).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryCodesRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.pagination = pagination_1.PageRequest.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryCodesRequest();
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageRequest.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageRequest.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryCodesRequest();
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageRequest.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryCodesResponse() {
      return {
        codeInfos: [],
        pagination: void 0
      };
    }
    exports.QueryCodesResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryCodesResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        for (const v of message.codeInfos) {
          exports.CodeInfoResponse.encode(v, writer.uint32(10).fork()).ldelim();
        }
        if (message.pagination !== void 0) {
          pagination_1.PageResponse.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryCodesResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeInfos.push(exports.CodeInfoResponse.decode(reader, reader.uint32()));
              break;
            case 2:
              message.pagination = pagination_1.PageResponse.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryCodesResponse();
        if (Array.isArray(object == null ? void 0 : object.codeInfos))
          obj.codeInfos = object.codeInfos.map((e) => exports.CodeInfoResponse.fromJSON(e));
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageResponse.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        if (message.codeInfos) {
          obj.codeInfos = message.codeInfos.map((e) => e ? exports.CodeInfoResponse.toJSON(e) : void 0);
        } else {
          obj.codeInfos = [];
        }
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageResponse.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseQueryCodesResponse();
        message.codeInfos = ((_a = object.codeInfos) == null ? void 0 : _a.map((e) => exports.CodeInfoResponse.fromPartial(e))) || [];
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageResponse.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryPinnedCodesRequest() {
      return {
        pagination: void 0
      };
    }
    exports.QueryPinnedCodesRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryPinnedCodesRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.pagination !== void 0) {
          pagination_1.PageRequest.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryPinnedCodesRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 2:
              message.pagination = pagination_1.PageRequest.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryPinnedCodesRequest();
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageRequest.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageRequest.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryPinnedCodesRequest();
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageRequest.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryPinnedCodesResponse() {
      return {
        codeIds: [],
        pagination: void 0
      };
    }
    exports.QueryPinnedCodesResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryPinnedCodesResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        writer.uint32(10).fork();
        for (const v of message.codeIds) {
          writer.uint64(v);
        }
        writer.ldelim();
        if (message.pagination !== void 0) {
          pagination_1.PageResponse.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryPinnedCodesResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              if ((tag & 7) === 2) {
                const end2 = reader.uint32() + reader.pos;
                while (reader.pos < end2) {
                  message.codeIds.push(reader.uint64());
                }
              } else {
                message.codeIds.push(reader.uint64());
              }
              break;
            case 2:
              message.pagination = pagination_1.PageResponse.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryPinnedCodesResponse();
        if (Array.isArray(object == null ? void 0 : object.codeIds))
          obj.codeIds = object.codeIds.map((e) => BigInt(e.toString()));
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageResponse.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        if (message.codeIds) {
          obj.codeIds = message.codeIds.map((e) => (e || BigInt(0)).toString());
        } else {
          obj.codeIds = [];
        }
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageResponse.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseQueryPinnedCodesResponse();
        message.codeIds = ((_a = object.codeIds) == null ? void 0 : _a.map((e) => BigInt(e.toString()))) || [];
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageResponse.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryParamsRequest() {
      return {};
    }
    exports.QueryParamsRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryParamsRequest",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryParamsRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseQueryParamsRequest();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseQueryParamsRequest();
        return message;
      }
    };
    function createBaseQueryParamsResponse() {
      return {
        params: types_1.Params.fromPartial({})
      };
    }
    exports.QueryParamsResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryParamsResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.params !== void 0) {
          types_1.Params.encode(message.params, writer.uint32(10).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryParamsResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.params = types_1.Params.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryParamsResponse();
        if ((0, helpers_1.isSet)(object.params))
          obj.params = types_1.Params.fromJSON(object.params);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.params !== void 0 && (obj.params = message.params ? types_1.Params.toJSON(message.params) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryParamsResponse();
        if (object.params !== void 0 && object.params !== null) {
          message.params = types_1.Params.fromPartial(object.params);
        }
        return message;
      }
    };
    function createBaseQueryContractsByCreatorRequest() {
      return {
        creatorAddress: "",
        pagination: void 0
      };
    }
    exports.QueryContractsByCreatorRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryContractsByCreatorRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.creatorAddress !== "") {
          writer.uint32(10).string(message.creatorAddress);
        }
        if (message.pagination !== void 0) {
          pagination_1.PageRequest.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryContractsByCreatorRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.creatorAddress = reader.string();
              break;
            case 2:
              message.pagination = pagination_1.PageRequest.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryContractsByCreatorRequest();
        if ((0, helpers_1.isSet)(object.creatorAddress))
          obj.creatorAddress = String(object.creatorAddress);
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageRequest.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.creatorAddress !== void 0 && (obj.creatorAddress = message.creatorAddress);
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageRequest.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryContractsByCreatorRequest();
        message.creatorAddress = object.creatorAddress ?? "";
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageRequest.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryContractsByCreatorResponse() {
      return {
        contractAddresses: [],
        pagination: void 0
      };
    }
    exports.QueryContractsByCreatorResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryContractsByCreatorResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        for (const v of message.contractAddresses) {
          writer.uint32(10).string(v);
        }
        if (message.pagination !== void 0) {
          pagination_1.PageResponse.encode(message.pagination, writer.uint32(18).fork()).ldelim();
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryContractsByCreatorResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.contractAddresses.push(reader.string());
              break;
            case 2:
              message.pagination = pagination_1.PageResponse.decode(reader, reader.uint32());
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryContractsByCreatorResponse();
        if (Array.isArray(object == null ? void 0 : object.contractAddresses))
          obj.contractAddresses = object.contractAddresses.map((e) => String(e));
        if ((0, helpers_1.isSet)(object.pagination))
          obj.pagination = pagination_1.PageResponse.fromJSON(object.pagination);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        if (message.contractAddresses) {
          obj.contractAddresses = message.contractAddresses.map((e) => e);
        } else {
          obj.contractAddresses = [];
        }
        message.pagination !== void 0 && (obj.pagination = message.pagination ? pagination_1.PageResponse.toJSON(message.pagination) : void 0);
        return obj;
      },
      fromPartial(object) {
        var _a;
        const message = createBaseQueryContractsByCreatorResponse();
        message.contractAddresses = ((_a = object.contractAddresses) == null ? void 0 : _a.map((e) => e)) || [];
        if (object.pagination !== void 0 && object.pagination !== null) {
          message.pagination = pagination_1.PageResponse.fromPartial(object.pagination);
        }
        return message;
      }
    };
    function createBaseQueryWasmLimitsConfigRequest() {
      return {};
    }
    exports.QueryWasmLimitsConfigRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryWasmLimitsConfigRequest",
      encode(_, writer = binary_1.BinaryWriter.create()) {
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryWasmLimitsConfigRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(_) {
        const obj = createBaseQueryWasmLimitsConfigRequest();
        return obj;
      },
      toJSON(_) {
        const obj = {};
        return obj;
      },
      fromPartial(_) {
        const message = createBaseQueryWasmLimitsConfigRequest();
        return message;
      }
    };
    function createBaseQueryWasmLimitsConfigResponse() {
      return {
        config: ""
      };
    }
    exports.QueryWasmLimitsConfigResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryWasmLimitsConfigResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.config !== "") {
          writer.uint32(10).string(message.config);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryWasmLimitsConfigResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.config = reader.string();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryWasmLimitsConfigResponse();
        if ((0, helpers_1.isSet)(object.config))
          obj.config = String(object.config);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.config !== void 0 && (obj.config = message.config);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryWasmLimitsConfigResponse();
        message.config = object.config ?? "";
        return message;
      }
    };
    function createBaseQueryBuildAddressRequest() {
      return {
        codeHash: "",
        creatorAddress: "",
        salt: "",
        initArgs: new Uint8Array()
      };
    }
    exports.QueryBuildAddressRequest = {
      typeUrl: "/cosmwasm.wasm.v1.QueryBuildAddressRequest",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.codeHash !== "") {
          writer.uint32(10).string(message.codeHash);
        }
        if (message.creatorAddress !== "") {
          writer.uint32(18).string(message.creatorAddress);
        }
        if (message.salt !== "") {
          writer.uint32(26).string(message.salt);
        }
        if (message.initArgs.length !== 0) {
          writer.uint32(34).bytes(message.initArgs);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryBuildAddressRequest();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.codeHash = reader.string();
              break;
            case 2:
              message.creatorAddress = reader.string();
              break;
            case 3:
              message.salt = reader.string();
              break;
            case 4:
              message.initArgs = reader.bytes();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryBuildAddressRequest();
        if ((0, helpers_1.isSet)(object.codeHash))
          obj.codeHash = String(object.codeHash);
        if ((0, helpers_1.isSet)(object.creatorAddress))
          obj.creatorAddress = String(object.creatorAddress);
        if ((0, helpers_1.isSet)(object.salt))
          obj.salt = String(object.salt);
        if ((0, helpers_1.isSet)(object.initArgs))
          obj.initArgs = (0, helpers_1.bytesFromBase64)(object.initArgs);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.codeHash !== void 0 && (obj.codeHash = message.codeHash);
        message.creatorAddress !== void 0 && (obj.creatorAddress = message.creatorAddress);
        message.salt !== void 0 && (obj.salt = message.salt);
        message.initArgs !== void 0 && (obj.initArgs = (0, helpers_1.base64FromBytes)(message.initArgs !== void 0 ? message.initArgs : new Uint8Array()));
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryBuildAddressRequest();
        message.codeHash = object.codeHash ?? "";
        message.creatorAddress = object.creatorAddress ?? "";
        message.salt = object.salt ?? "";
        message.initArgs = object.initArgs ?? new Uint8Array();
        return message;
      }
    };
    function createBaseQueryBuildAddressResponse() {
      return {
        address: ""
      };
    }
    exports.QueryBuildAddressResponse = {
      typeUrl: "/cosmwasm.wasm.v1.QueryBuildAddressResponse",
      encode(message, writer = binary_1.BinaryWriter.create()) {
        if (message.address !== "") {
          writer.uint32(10).string(message.address);
        }
        return writer;
      },
      decode(input, length) {
        const reader = input instanceof binary_1.BinaryReader ? input : new binary_1.BinaryReader(input);
        let end = length === void 0 ? reader.len : reader.pos + length;
        const message = createBaseQueryBuildAddressResponse();
        while (reader.pos < end) {
          const tag = reader.uint32();
          switch (tag >>> 3) {
            case 1:
              message.address = reader.string();
              break;
            default:
              reader.skipType(tag & 7);
              break;
          }
        }
        return message;
      },
      fromJSON(object) {
        const obj = createBaseQueryBuildAddressResponse();
        if ((0, helpers_1.isSet)(object.address))
          obj.address = String(object.address);
        return obj;
      },
      toJSON(message) {
        const obj = {};
        message.address !== void 0 && (obj.address = message.address);
        return obj;
      },
      fromPartial(object) {
        const message = createBaseQueryBuildAddressResponse();
        message.address = object.address ?? "";
        return message;
      }
    };
    var QueryClientImpl = class {
      constructor(rpc) {
        this.rpc = rpc;
        this.ContractInfo = this.ContractInfo.bind(this);
        this.ContractHistory = this.ContractHistory.bind(this);
        this.ContractsByCode = this.ContractsByCode.bind(this);
        this.AllContractState = this.AllContractState.bind(this);
        this.RawContractState = this.RawContractState.bind(this);
        this.SmartContractState = this.SmartContractState.bind(this);
        this.Code = this.Code.bind(this);
        this.Codes = this.Codes.bind(this);
        this.CodeInfo = this.CodeInfo.bind(this);
        this.PinnedCodes = this.PinnedCodes.bind(this);
        this.Params = this.Params.bind(this);
        this.ContractsByCreator = this.ContractsByCreator.bind(this);
        this.WasmLimitsConfig = this.WasmLimitsConfig.bind(this);
        this.BuildAddress = this.BuildAddress.bind(this);
      }
      ContractInfo(request) {
        const data = exports.QueryContractInfoRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "ContractInfo", data);
        return promise.then((data2) => exports.QueryContractInfoResponse.decode(new binary_1.BinaryReader(data2)));
      }
      ContractHistory(request) {
        const data = exports.QueryContractHistoryRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "ContractHistory", data);
        return promise.then((data2) => exports.QueryContractHistoryResponse.decode(new binary_1.BinaryReader(data2)));
      }
      ContractsByCode(request) {
        const data = exports.QueryContractsByCodeRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "ContractsByCode", data);
        return promise.then((data2) => exports.QueryContractsByCodeResponse.decode(new binary_1.BinaryReader(data2)));
      }
      AllContractState(request) {
        const data = exports.QueryAllContractStateRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "AllContractState", data);
        return promise.then((data2) => exports.QueryAllContractStateResponse.decode(new binary_1.BinaryReader(data2)));
      }
      RawContractState(request) {
        const data = exports.QueryRawContractStateRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "RawContractState", data);
        return promise.then((data2) => exports.QueryRawContractStateResponse.decode(new binary_1.BinaryReader(data2)));
      }
      SmartContractState(request) {
        const data = exports.QuerySmartContractStateRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "SmartContractState", data);
        return promise.then((data2) => exports.QuerySmartContractStateResponse.decode(new binary_1.BinaryReader(data2)));
      }
      Code(request) {
        const data = exports.QueryCodeRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "Code", data);
        return promise.then((data2) => exports.QueryCodeResponse.decode(new binary_1.BinaryReader(data2)));
      }
      Codes(request = {
        pagination: pagination_1.PageRequest.fromPartial({})
      }) {
        const data = exports.QueryCodesRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "Codes", data);
        return promise.then((data2) => exports.QueryCodesResponse.decode(new binary_1.BinaryReader(data2)));
      }
      CodeInfo(request) {
        const data = exports.QueryCodeInfoRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "CodeInfo", data);
        return promise.then((data2) => exports.QueryCodeInfoResponse.decode(new binary_1.BinaryReader(data2)));
      }
      PinnedCodes(request = {
        pagination: pagination_1.PageRequest.fromPartial({})
      }) {
        const data = exports.QueryPinnedCodesRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "PinnedCodes", data);
        return promise.then((data2) => exports.QueryPinnedCodesResponse.decode(new binary_1.BinaryReader(data2)));
      }
      Params(request = {}) {
        const data = exports.QueryParamsRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "Params", data);
        return promise.then((data2) => exports.QueryParamsResponse.decode(new binary_1.BinaryReader(data2)));
      }
      ContractsByCreator(request) {
        const data = exports.QueryContractsByCreatorRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "ContractsByCreator", data);
        return promise.then((data2) => exports.QueryContractsByCreatorResponse.decode(new binary_1.BinaryReader(data2)));
      }
      WasmLimitsConfig(request = {}) {
        const data = exports.QueryWasmLimitsConfigRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "WasmLimitsConfig", data);
        return promise.then((data2) => exports.QueryWasmLimitsConfigResponse.decode(new binary_1.BinaryReader(data2)));
      }
      BuildAddress(request) {
        const data = exports.QueryBuildAddressRequest.encode(request).finish();
        const promise = this.rpc.request("cosmwasm.wasm.v1.Query", "BuildAddress", data);
        return promise.then((data2) => exports.QueryBuildAddressResponse.decode(new binary_1.BinaryReader(data2)));
      }
    };
    exports.QueryClientImpl = QueryClientImpl;
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/modules/wasm/queries.js
var require_queries = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/modules/wasm/queries.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.setupWasmExtension = setupWasmExtension;
    var encoding_1 = require_build3();
    var stargate_1 = require_build8();
    var query_1 = require_query();
    function setupWasmExtension(base) {
      const rpc = (0, stargate_1.createProtobufRpcClient)(base);
      const queryService = new query_1.QueryClientImpl(rpc);
      return {
        wasm: {
          listCodeInfo: async (paginationKey) => {
            const request = {
              pagination: (0, stargate_1.createPagination)(paginationKey)
            };
            return queryService.Codes(request);
          },
          getCode: async (id) => {
            const request = query_1.QueryCodeRequest.fromPartial({ codeId: BigInt(id) });
            return queryService.Code(request);
          },
          listContractsByCodeId: async (id, paginationKey) => {
            const request = query_1.QueryContractsByCodeRequest.fromPartial({
              codeId: BigInt(id),
              pagination: (0, stargate_1.createPagination)(paginationKey)
            });
            return queryService.ContractsByCode(request);
          },
          listContractsByCreator: async (creator, paginationKey) => {
            const request = {
              creatorAddress: creator,
              pagination: (0, stargate_1.createPagination)(paginationKey)
            };
            return queryService.ContractsByCreator(request);
          },
          getContractInfo: async (address) => {
            const request = { address };
            return queryService.ContractInfo(request);
          },
          getContractCodeHistory: async (address, paginationKey) => {
            const request = {
              address,
              pagination: (0, stargate_1.createPagination)(paginationKey)
            };
            return queryService.ContractHistory(request);
          },
          getAllContractState: async (address, paginationKey) => {
            const request = {
              address,
              pagination: (0, stargate_1.createPagination)(paginationKey)
            };
            return queryService.AllContractState(request);
          },
          queryContractRaw: async (address, key) => {
            const request = { address, queryData: key };
            return queryService.RawContractState(request);
          },
          queryContractSmart: async (address, query) => {
            const request = { address, queryData: (0, encoding_1.toUtf8)(JSON.stringify(query)) };
            const { data } = await queryService.SmartContractState(request);
            let responseText;
            try {
              responseText = (0, encoding_1.fromUtf8)(data);
            } catch (error) {
              throw new Error(`Could not UTF-8 decode smart query response from contract: ${error}`);
            }
            try {
              return JSON.parse(responseText);
            } catch (error) {
              throw new Error(`Could not JSON parse smart query response from contract: ${error}`);
            }
          }
        }
      };
    }
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/modules/index.js
var require_modules = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/modules/index.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.setupWasmExtension = exports.wasmTypes = exports.isMsgUpdateAdminEncodeObject = exports.isMsgStoreCodeEncodeObject = exports.isMsgMigrateEncodeObject = exports.isMsgInstantiateContractEncodeObject = exports.isMsgInstantiateContract2EncodeObject = exports.isMsgExecuteEncodeObject = exports.isMsgClearAdminEncodeObject = exports.createWasmAminoConverters = void 0;
    var aminomessages_1 = require_aminomessages();
    Object.defineProperty(exports, "createWasmAminoConverters", { enumerable: true, get: function() {
      return aminomessages_1.createWasmAminoConverters;
    } });
    var messages_1 = require_messages();
    Object.defineProperty(exports, "isMsgClearAdminEncodeObject", { enumerable: true, get: function() {
      return messages_1.isMsgClearAdminEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgExecuteEncodeObject", { enumerable: true, get: function() {
      return messages_1.isMsgExecuteEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgInstantiateContract2EncodeObject", { enumerable: true, get: function() {
      return messages_1.isMsgInstantiateContract2EncodeObject;
    } });
    Object.defineProperty(exports, "isMsgInstantiateContractEncodeObject", { enumerable: true, get: function() {
      return messages_1.isMsgInstantiateContractEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgMigrateEncodeObject", { enumerable: true, get: function() {
      return messages_1.isMsgMigrateEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgStoreCodeEncodeObject", { enumerable: true, get: function() {
      return messages_1.isMsgStoreCodeEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgUpdateAdminEncodeObject", { enumerable: true, get: function() {
      return messages_1.isMsgUpdateAdminEncodeObject;
    } });
    Object.defineProperty(exports, "wasmTypes", { enumerable: true, get: function() {
      return messages_1.wasmTypes;
    } });
    var queries_1 = require_queries();
    Object.defineProperty(exports, "setupWasmExtension", { enumerable: true, get: function() {
      return queries_1.setupWasmExtension;
    } });
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/cosmwasmclient.js
var require_cosmwasmclient = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/cosmwasmclient.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.CosmWasmClient = void 0;
    var encoding_1 = require_build3();
    var math_1 = require_build();
    var stargate_1 = require_build8();
    var tendermint_rpc_1 = require_build7();
    var utils_1 = require_build2();
    var abci_1 = require_abci();
    var types_1 = require_types();
    var modules_1 = require_modules();
    var CosmWasmClient = class _CosmWasmClient {
      constructor(cometClient) {
        __publicField(this, "cometClient");
        __publicField(this, "queryClient");
        __publicField(this, "codesCache", /* @__PURE__ */ new Map());
        __publicField(this, "chainId");
        if (cometClient) {
          this.cometClient = cometClient;
          this.queryClient = stargate_1.QueryClient.withExtensions(cometClient, stargate_1.setupAuthExtension, stargate_1.setupBankExtension, modules_1.setupWasmExtension, stargate_1.setupTxExtension);
        }
      }
      /**
       * Creates an instance by connecting to the given CometBFT RPC endpoint.
       *
       * This uses auto-detection to decide between a CometBFT 0.38, Tendermint 0.37 and 0.34 client.
       * To set the Comet client explicitly, use `create`.
       */
      static async connect(endpoint) {
        const cometClient = await (0, tendermint_rpc_1.connectComet)(endpoint);
        return _CosmWasmClient.create(cometClient);
      }
      /**
       * Creates an instance from a manually created Comet client.
       * Use this to use `Comet38Client` or `Tendermint37Client` instead of `Tendermint34Client`.
       */
      static create(cometClient) {
        return new _CosmWasmClient(cometClient);
      }
      getCometClient() {
        return this.cometClient;
      }
      forceGetCometClient() {
        if (!this.cometClient) {
          throw new Error("Comet client not available. You cannot use online functionality in offline mode.");
        }
        return this.cometClient;
      }
      getQueryClient() {
        return this.queryClient;
      }
      forceGetQueryClient() {
        if (!this.queryClient) {
          throw new Error("Query client not available. You cannot use online functionality in offline mode.");
        }
        return this.queryClient;
      }
      async getChainId() {
        if (!this.chainId) {
          const response = await this.forceGetCometClient().status();
          const chainId = response.nodeInfo.network;
          if (!chainId)
            throw new Error("Chain ID must not be empty");
          this.chainId = chainId;
        }
        return this.chainId;
      }
      async getHeight() {
        const status = await this.forceGetCometClient().status();
        return status.syncInfo.latestBlockHeight;
      }
      async getAccount(searchAddress) {
        try {
          const account = await this.forceGetQueryClient().auth.account(searchAddress);
          return account ? (0, stargate_1.accountFromAny)(account) : null;
        } catch (error) {
          if (/rpc error: code = NotFound/i.test(error.toString())) {
            return null;
          }
          throw error;
        }
      }
      async getSequence(address) {
        const account = await this.getAccount(address);
        if (!account) {
          throw new Error(`Account '${address}' does not exist on chain. Send some tokens there before trying to query sequence.`);
        }
        return {
          accountNumber: account.accountNumber,
          sequence: account.sequence
        };
      }
      async getBlock(height) {
        const response = await this.forceGetCometClient().block(height);
        return {
          id: (0, encoding_1.toHex)(response.blockId.hash).toUpperCase(),
          header: {
            version: {
              block: new math_1.Uint53(response.block.header.version.block).toString(),
              app: new math_1.Uint53(response.block.header.version.app).toString()
            },
            height: response.block.header.height,
            chainId: response.block.header.chainId,
            time: (0, tendermint_rpc_1.toRfc3339WithNanoseconds)(response.block.header.time)
          },
          txs: response.block.txs
        };
      }
      async getBalance(address, searchDenom) {
        return this.forceGetQueryClient().bank.balance(address, searchDenom);
      }
      async getTx(id) {
        const results = await this.txsQuery(`tx.hash='${id}'`);
        return results[0] ?? null;
      }
      async searchTx(query) {
        let rawQuery;
        if (typeof query === "string") {
          rawQuery = query;
        } else if ((0, stargate_1.isSearchTxQueryArray)(query)) {
          rawQuery = query.map((t) => {
            if (typeof t.value === "string")
              return `${t.key}='${t.value}'`;
            else
              return `${t.key}=${t.value}`;
          }).join(" AND ");
        } else {
          throw new Error("Got unsupported query type. See CosmJS 0.31 CHANGELOG for API breaking changes here.");
        }
        return this.txsQuery(rawQuery);
      }
      disconnect() {
        if (this.cometClient)
          this.cometClient.disconnect();
      }
      /**
       * Broadcasts a signed transaction to the network and monitors its inclusion in a block.
       *
       * If broadcasting is rejected by the node for some reason (e.g. because of a CheckTx failure),
       * an error is thrown.
       *
       * If the transaction is not included in a block before the provided timeout, this errors with a `TimeoutError`.
       *
       * If the transaction is included in a block, a `DeliverTxResponse` is returned. The caller then
       * usually needs to check for execution success or failure.
       */
      // NOTE: This method is tested against slow chains and timeouts in the @cosmjs/stargate package.
      // Make sure it is kept in sync!
      async broadcastTx(tx, timeoutMs = 6e4, pollIntervalMs = 3e3) {
        let timedOut = false;
        const txPollTimeout = setTimeout(() => {
          timedOut = true;
        }, timeoutMs);
        const pollForTx = async (txId) => {
          if (timedOut) {
            throw new stargate_1.TimeoutError(`Transaction with ID ${txId} was submitted but was not yet found on the chain. You might want to check later. There was a wait of ${timeoutMs / 1e3} seconds.`, txId);
          }
          await (0, utils_1.sleep)(pollIntervalMs);
          const result = await this.getTx(txId);
          return result ? {
            code: result.code,
            height: result.height,
            txIndex: result.txIndex,
            rawLog: result.rawLog,
            transactionHash: txId,
            events: result.events,
            msgResponses: result.msgResponses,
            gasUsed: result.gasUsed,
            gasWanted: result.gasWanted
          } : pollForTx(txId);
        };
        const transactionId = await this.broadcastTxSync(tx);
        return new Promise((resolve, reject) => pollForTx(transactionId).then((value) => {
          clearTimeout(txPollTimeout);
          resolve(value);
        }, (error) => {
          clearTimeout(txPollTimeout);
          reject(error);
        }));
      }
      /**
       * Broadcasts a signed transaction to the network without monitoring it.
       *
       * If broadcasting is rejected by the node for some reason (e.g. because of a CheckTx failure),
       * an error is thrown.
       *
       * If the transaction is broadcasted, a `string` containing the hash of the transaction is returned. The caller then
       * usually needs to check if the transaction was included in a block and was successful.
       *
       * @returns Returns the hash of the transaction
       */
      async broadcastTxSync(tx) {
        const broadcasted = await this.forceGetCometClient().broadcastTxSync({ tx });
        if (broadcasted.code) {
          return Promise.reject(new stargate_1.BroadcastTxError(broadcasted.code, broadcasted.codespace ?? "", broadcasted.log));
        }
        const transactionId = (0, encoding_1.toHex)(broadcasted.hash).toUpperCase();
        return transactionId;
      }
      /**
       * getCodes() returns all codes and is just looping through all pagination pages.
       *
       * This is potentially inefficient and advanced apps should consider creating
       * their own query client to handle pagination together with the app's screens.
       */
      async getCodes() {
        const allCodes = [];
        let startAtKey = void 0;
        do {
          const { codeInfos, pagination } = await this.forceGetQueryClient().wasm.listCodeInfo(startAtKey);
          const loadedCodes = codeInfos || [];
          allCodes.push(...loadedCodes);
          startAtKey = pagination == null ? void 0 : pagination.nextKey;
        } while ((startAtKey == null ? void 0 : startAtKey.length) !== 0);
        return allCodes.map((entry) => {
          (0, utils_1.assert)(entry.creator && entry.codeId && entry.dataHash, "entry incomplete");
          return {
            id: Number(entry.codeId),
            creator: entry.creator,
            checksum: (0, encoding_1.toHex)(entry.dataHash)
          };
        });
      }
      async getCodeDetails(codeId) {
        const cached = this.codesCache.get(codeId);
        if (cached)
          return cached;
        const { codeInfo, data } = await this.forceGetQueryClient().wasm.getCode(codeId);
        (0, utils_1.assert)(codeInfo && codeInfo.codeId && codeInfo.creator && codeInfo.dataHash && data, "codeInfo missing or incomplete");
        const codeDetails = {
          id: Number(codeInfo.codeId),
          creator: codeInfo.creator,
          checksum: (0, encoding_1.toHex)(codeInfo.dataHash),
          data
        };
        this.codesCache.set(codeId, codeDetails);
        return codeDetails;
      }
      /**
       * getContracts() returns all contract instances for one code and is just looping through all pagination pages.
       *
       * This is potentially inefficient and advanced apps should consider creating
       * their own query client to handle pagination together with the app's screens.
       */
      async getContracts(codeId) {
        const allContracts = [];
        let startAtKey = void 0;
        do {
          const { contracts, pagination } = await this.forceGetQueryClient().wasm.listContractsByCodeId(codeId, startAtKey);
          allContracts.push(...contracts);
          startAtKey = pagination == null ? void 0 : pagination.nextKey;
        } while ((startAtKey == null ? void 0 : startAtKey.length) !== 0 && startAtKey !== void 0);
        return allContracts;
      }
      /**
       * Returns a list of contract addresses created by the given creator.
       * This just loops through all pagination pages.
       */
      async getContractsByCreator(creator) {
        const allContracts = [];
        let startAtKey = void 0;
        do {
          const { contractAddresses, pagination } = await this.forceGetQueryClient().wasm.listContractsByCreator(creator, startAtKey);
          allContracts.push(...contractAddresses);
          startAtKey = pagination == null ? void 0 : pagination.nextKey;
        } while ((startAtKey == null ? void 0 : startAtKey.length) !== 0 && startAtKey !== void 0);
        return allContracts;
      }
      /**
       * Throws an error if no contract was found at the address
       */
      async getContract(address) {
        const { address: retrievedAddress, contractInfo } = await this.forceGetQueryClient().wasm.getContractInfo(address);
        if (!contractInfo)
          throw new Error(`No contract found at address "${address}"`);
        (0, utils_1.assert)(retrievedAddress, "address missing");
        (0, utils_1.assert)(contractInfo.codeId && contractInfo.creator && contractInfo.label, "contractInfo incomplete");
        return {
          address: retrievedAddress,
          codeId: Number(contractInfo.codeId),
          creator: contractInfo.creator,
          admin: contractInfo.admin || void 0,
          label: contractInfo.label,
          ibcPortId: contractInfo.ibcPortId || void 0
        };
      }
      /**
       * Throws an error if no contract was found at the address
       */
      async getContractCodeHistory(address) {
        const result = await this.forceGetQueryClient().wasm.getContractCodeHistory(address);
        if (!result)
          throw new Error(`No contract history found for address "${address}"`);
        const operations = {
          [types_1.ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_INIT]: "Init",
          [types_1.ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_GENESIS]: "Genesis",
          [types_1.ContractCodeHistoryOperationType.CONTRACT_CODE_HISTORY_OPERATION_TYPE_MIGRATE]: "Migrate"
        };
        return (result.entries || []).map((entry) => {
          (0, utils_1.assert)(entry.operation && entry.codeId && entry.msg);
          return {
            operation: operations[entry.operation],
            codeId: Number(entry.codeId),
            msg: JSON.parse((0, encoding_1.fromUtf8)(entry.msg))
          };
        });
      }
      /**
       * Returns the data at the key if present (raw contract dependent storage data)
       * or null if no data at this key.
       *
       * Promise is rejected when contract does not exist.
       */
      async queryContractRaw(address, key) {
        await this.getContract(address);
        const { data } = await this.forceGetQueryClient().wasm.queryContractRaw(address, key);
        return data ?? null;
      }
      /**
       * Makes a smart query on the contract, returns the parsed JSON document.
       *
       * Promise is rejected when contract does not exist.
       * Promise is rejected for invalid query format.
       * Promise is rejected for invalid response format.
       */
      async queryContractSmart(address, queryMsg) {
        try {
          return await this.forceGetQueryClient().wasm.queryContractSmart(address, queryMsg);
        } catch (error) {
          if (error instanceof Error) {
            if (error.message.startsWith("not found: contract")) {
              throw new Error(`No contract found at address "${address}"`);
            } else {
              throw error;
            }
          } else {
            throw error;
          }
        }
      }
      async txsQuery(query) {
        const results = await this.forceGetCometClient().txSearchAll({ query });
        return results.txs.map((tx) => {
          const txMsgData = abci_1.TxMsgData.decode(tx.result.data ?? new Uint8Array());
          return {
            height: tx.height,
            txIndex: tx.index,
            hash: (0, encoding_1.toHex)(tx.hash).toUpperCase(),
            code: tx.result.code,
            events: tx.result.events.map(stargate_1.fromTendermintEvent),
            rawLog: tx.result.log || "",
            tx: tx.tx,
            msgResponses: txMsgData.msgResponses,
            gasUsed: tx.result.gasUsed,
            gasWanted: tx.result.gasWanted
          };
        });
      }
    };
    exports.CosmWasmClient = CosmWasmClient;
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/encoding.js
var require_encoding = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/encoding.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.toBinary = toBinary;
    exports.fromBinary = fromBinary;
    var encoding_1 = require_build3();
    function toBinary(obj) {
      return (0, encoding_1.toBase64)((0, encoding_1.toUtf8)(JSON.stringify(obj)));
    }
    function fromBinary(base64) {
      return JSON.parse((0, encoding_1.fromUtf8)((0, encoding_1.fromBase64)(base64)));
    }
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/instantiate2.js
var require_instantiate2 = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/instantiate2.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports._instantiate2AddressIntermediate = _instantiate2AddressIntermediate;
    exports.instantiate2Address = instantiate2Address;
    var crypto_1 = require_build4();
    var encoding_1 = require_build3();
    var math_1 = require_build();
    var utils_1 = require_build2();
    function hash(type, key) {
      return new crypto_1.Sha256((0, crypto_1.sha256)((0, encoding_1.toAscii)(type))).update(key).digest();
    }
    function toUint64(int) {
      return math_1.Uint64.fromNumber(int).toBytesBigEndian();
    }
    function _instantiate2AddressIntermediate(checksum, creator, salt, msg, prefix) {
      (0, utils_1.assert)(checksum.length === 32);
      const creatorData = (0, encoding_1.fromBech32)(creator).data;
      const msgData = typeof msg === "string" ? (0, encoding_1.toUtf8)(msg) : new Uint8Array();
      if (salt.length < 1 || salt.length > 64)
        throw new Error("Salt must be between 1 and 64 bytes");
      const key = new Uint8Array([
        ...(0, encoding_1.toAscii)("wasm"),
        0,
        ...toUint64(checksum.length),
        ...checksum,
        ...toUint64(creatorData.length),
        ...creatorData,
        ...toUint64(salt.length),
        ...salt,
        ...toUint64(msgData.length),
        ...msgData
      ]);
      const addressData = hash("module", key);
      const address = (0, encoding_1.toBech32)(prefix, addressData);
      return { key, addressData, address };
    }
    function instantiate2Address(checksum, creator, salt, bech32Prefix) {
      const msg = null;
      return _instantiate2AddressIntermediate(checksum, creator, salt, msg, bech32Prefix).address;
    }
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/compression.js
var require_compression = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/compression.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.gzip = gzip;
    async function gzip(uncompressed) {
      const inputStream = new Blob([uncompressed]).stream();
      const stream = inputStream.pipeThrough(new CompressionStream("gzip"));
      const buffer = await new Response(stream).arrayBuffer();
      return new Uint8Array(buffer);
    }
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/signingcosmwasmclient.js
var require_signingcosmwasmclient = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/signingcosmwasmclient.js"(exports) {
    "use strict";
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.SigningCosmWasmClient = void 0;
    exports.findAttribute = findAttribute;
    var amino_1 = require_build5();
    var crypto_1 = require_build4();
    var encoding_1 = require_build3();
    var math_1 = require_build();
    var proto_signing_1 = require_build6();
    var stargate_1 = require_build8();
    var tendermint_rpc_1 = require_build7();
    var utils_1 = require_build2();
    var tx_1 = require_tx2();
    var tx_2 = require_tx3();
    var signing_1 = require_signing();
    var tx_3 = require_tx();
    var tx_4 = require_tx4();
    var compression_1 = require_compression();
    var cosmwasmclient_1 = require_cosmwasmclient();
    var modules_1 = require_modules();
    function findAttribute(events, eventType, attrKey) {
      const attributes = events.filter((event) => event.type === eventType).flatMap((e) => e.attributes);
      const out = attributes.find((attr) => attr.key === attrKey);
      if (!out) {
        throw new Error(`Could not find attribute '${attrKey}' in first event of type '${eventType}' in first log.`);
      }
      return out;
    }
    function createDeliverTxResponseErrorMessage(result) {
      return `Error when broadcasting tx ${result.transactionHash} at height ${result.height}. Code: ${result.code}; Raw log: ${result.rawLog}`;
    }
    var SigningCosmWasmClient = class _SigningCosmWasmClient extends cosmwasmclient_1.CosmWasmClient {
      constructor(cometClient, signer, options) {
        super(cometClient);
        __publicField(this, "registry");
        __publicField(this, "broadcastTimeoutMs");
        __publicField(this, "broadcastPollIntervalMs");
        __publicField(this, "signer");
        __publicField(this, "aminoTypes");
        __publicField(this, "gasPrice");
        // Starting with Cosmos SDK 0.47, we see many cases in which 1.3 is not enough anymore
        // E.g. https://github.com/cosmos/cosmos-sdk/issues/16020
        __publicField(this, "defaultGasMultiplier", 1.4);
        const { registry = new proto_signing_1.Registry([...stargate_1.defaultRegistryTypes, ...modules_1.wasmTypes]), aminoTypes = new stargate_1.AminoTypes({
          ...(0, stargate_1.createDefaultAminoConverters)(),
          ...(0, modules_1.createWasmAminoConverters)()
        }) } = options;
        this.registry = registry;
        this.aminoTypes = aminoTypes;
        this.signer = signer;
        this.broadcastTimeoutMs = options.broadcastTimeoutMs;
        this.broadcastPollIntervalMs = options.broadcastPollIntervalMs;
        this.gasPrice = options.gasPrice;
      }
      /**
       * Creates an instance by connecting to the given CometBFT RPC endpoint.
       *
       * This uses auto-detection to decide between a CometBFT 0.38, Tendermint 0.37 and 0.34 client.
       * To set the Comet client explicitly, use `createWithSigner`.
       */
      static async connectWithSigner(endpoint, signer, options = {}) {
        const cometClient = await (0, tendermint_rpc_1.connectComet)(endpoint);
        return _SigningCosmWasmClient.createWithSigner(cometClient, signer, options);
      }
      /**
       * Creates an instance from a manually created Comet client.
       * Use this to use `Comet38Client` or `Tendermint37Client` instead of `Tendermint34Client`.
       */
      static createWithSigner(cometClient, signer, options = {}) {
        return new _SigningCosmWasmClient(cometClient, signer, options);
      }
      /**
       * Creates a client in offline mode.
       *
       * This should only be used in niche cases where you know exactly what you're doing,
       * e.g. when building an offline signing application.
       *
       * When you try to use online functionality with such a signer, an
       * exception will be raised.
       */
      static async offline(signer, options = {}) {
        return new _SigningCosmWasmClient(void 0, signer, options);
      }
      async simulate(signerAddress, messages, memo) {
        const anyMsgs = messages.map((m) => this.registry.encodeAsAny(m));
        const accountFromSigner = (await this.signer.getAccounts()).find((account) => account.address === signerAddress);
        if (!accountFromSigner) {
          throw new Error("Failed to retrieve account from signer");
        }
        const pubkey = (0, amino_1.encodeSecp256k1Pubkey)(accountFromSigner.pubkey);
        const { sequence } = await this.getSequence(signerAddress);
        const { gasInfo } = await this.forceGetQueryClient().tx.simulate(anyMsgs, memo, pubkey, sequence);
        (0, utils_1.assertDefined)(gasInfo);
        return math_1.Uint53.fromString(gasInfo.gasUsed.toString()).toNumber();
      }
      /** Uploads code and returns a receipt, including the code ID */
      async upload(senderAddress, wasmCode, fee, memo = "", instantiatePermission) {
        const compressed = await (0, compression_1.gzip)(wasmCode);
        const storeCodeMsg = {
          typeUrl: "/cosmwasm.wasm.v1.MsgStoreCode",
          value: tx_4.MsgStoreCode.fromPartial({
            sender: senderAddress,
            wasmByteCode: compressed,
            instantiatePermission
          })
        };
        const usedFee = fee == "auto" ? 1.1 : fee;
        const result = await this.signAndBroadcast(senderAddress, [storeCodeMsg], usedFee, memo);
        if ((0, stargate_1.isDeliverTxFailure)(result)) {
          throw new Error(createDeliverTxResponseErrorMessage(result));
        }
        const codeIdAttr = findAttribute(result.events, "store_code", "code_id");
        return {
          checksum: (0, encoding_1.toHex)((0, crypto_1.sha256)(wasmCode)),
          originalSize: wasmCode.length,
          compressedSize: compressed.length,
          codeId: Number.parseInt(codeIdAttr.value, 10),
          logs: stargate_1.logs.parseRawLog(result.rawLog),
          height: result.height,
          transactionHash: result.transactionHash,
          events: result.events,
          gasWanted: result.gasWanted,
          gasUsed: result.gasUsed
        };
      }
      async instantiate(senderAddress, codeId, msg, label, fee, options = {}) {
        const instantiateContractMsg = {
          typeUrl: "/cosmwasm.wasm.v1.MsgInstantiateContract",
          value: tx_4.MsgInstantiateContract.fromPartial({
            sender: senderAddress,
            codeId: BigInt(new math_1.Uint53(codeId).toString()),
            label,
            msg: (0, encoding_1.toUtf8)(JSON.stringify(msg)),
            funds: [...options.funds || []],
            admin: options.admin
          })
        };
        const result = await this.signAndBroadcast(senderAddress, [instantiateContractMsg], fee, options.memo);
        if ((0, stargate_1.isDeliverTxFailure)(result)) {
          throw new Error(createDeliverTxResponseErrorMessage(result));
        }
        const contractAddressAttr = findAttribute(result.events, "instantiate", "_contract_address");
        return {
          contractAddress: contractAddressAttr.value,
          logs: stargate_1.logs.parseRawLog(result.rawLog),
          height: result.height,
          transactionHash: result.transactionHash,
          events: result.events,
          gasWanted: result.gasWanted,
          gasUsed: result.gasUsed
        };
      }
      async instantiate2(senderAddress, codeId, salt, msg, label, fee, options = {}) {
        const instantiateContract2Msg = {
          typeUrl: "/cosmwasm.wasm.v1.MsgInstantiateContract2",
          value: tx_4.MsgInstantiateContract2.fromPartial({
            sender: senderAddress,
            codeId: BigInt(new math_1.Uint53(codeId).toString()),
            label,
            msg: (0, encoding_1.toUtf8)(JSON.stringify(msg)),
            funds: [...options.funds || []],
            admin: options.admin,
            salt,
            fixMsg: false
          })
        };
        const result = await this.signAndBroadcast(senderAddress, [instantiateContract2Msg], fee, options.memo);
        if ((0, stargate_1.isDeliverTxFailure)(result)) {
          throw new Error(createDeliverTxResponseErrorMessage(result));
        }
        const contractAddressAttr = findAttribute(result.events, "instantiate", "_contract_address");
        return {
          contractAddress: contractAddressAttr.value,
          logs: stargate_1.logs.parseRawLog(result.rawLog),
          height: result.height,
          transactionHash: result.transactionHash,
          events: result.events,
          gasWanted: result.gasWanted,
          gasUsed: result.gasUsed
        };
      }
      async updateAdmin(senderAddress, contractAddress, newAdmin, fee, memo = "") {
        const updateAdminMsg = {
          typeUrl: "/cosmwasm.wasm.v1.MsgUpdateAdmin",
          value: tx_4.MsgUpdateAdmin.fromPartial({
            sender: senderAddress,
            contract: contractAddress,
            newAdmin
          })
        };
        const result = await this.signAndBroadcast(senderAddress, [updateAdminMsg], fee, memo);
        if ((0, stargate_1.isDeliverTxFailure)(result)) {
          throw new Error(createDeliverTxResponseErrorMessage(result));
        }
        return {
          logs: stargate_1.logs.parseRawLog(result.rawLog),
          height: result.height,
          transactionHash: result.transactionHash,
          events: result.events,
          gasWanted: result.gasWanted,
          gasUsed: result.gasUsed
        };
      }
      async clearAdmin(senderAddress, contractAddress, fee, memo = "") {
        const clearAdminMsg = {
          typeUrl: "/cosmwasm.wasm.v1.MsgClearAdmin",
          value: tx_4.MsgClearAdmin.fromPartial({
            sender: senderAddress,
            contract: contractAddress
          })
        };
        const result = await this.signAndBroadcast(senderAddress, [clearAdminMsg], fee, memo);
        if ((0, stargate_1.isDeliverTxFailure)(result)) {
          throw new Error(createDeliverTxResponseErrorMessage(result));
        }
        return {
          logs: stargate_1.logs.parseRawLog(result.rawLog),
          height: result.height,
          transactionHash: result.transactionHash,
          events: result.events,
          gasWanted: result.gasWanted,
          gasUsed: result.gasUsed
        };
      }
      async migrate(senderAddress, contractAddress, codeId, migrateMsg, fee, memo = "") {
        const migrateContractMsg = {
          typeUrl: "/cosmwasm.wasm.v1.MsgMigrateContract",
          value: tx_4.MsgMigrateContract.fromPartial({
            sender: senderAddress,
            contract: contractAddress,
            codeId: BigInt(new math_1.Uint53(codeId).toString()),
            msg: (0, encoding_1.toUtf8)(JSON.stringify(migrateMsg))
          })
        };
        const result = await this.signAndBroadcast(senderAddress, [migrateContractMsg], fee, memo);
        if ((0, stargate_1.isDeliverTxFailure)(result)) {
          throw new Error(createDeliverTxResponseErrorMessage(result));
        }
        return {
          logs: stargate_1.logs.parseRawLog(result.rawLog),
          height: result.height,
          transactionHash: result.transactionHash,
          events: result.events,
          gasWanted: result.gasWanted,
          gasUsed: result.gasUsed
        };
      }
      async execute(senderAddress, contractAddress, msg, fee, memo = "", funds) {
        const instruction = {
          contractAddress,
          msg,
          funds
        };
        return this.executeMultiple(senderAddress, [instruction], fee, memo);
      }
      /**
       * Like `execute` but allows executing multiple messages in one transaction.
       */
      async executeMultiple(senderAddress, instructions, fee, memo = "") {
        const msgs = instructions.map((i) => ({
          typeUrl: "/cosmwasm.wasm.v1.MsgExecuteContract",
          value: tx_4.MsgExecuteContract.fromPartial({
            sender: senderAddress,
            contract: i.contractAddress,
            msg: (0, encoding_1.toUtf8)(JSON.stringify(i.msg)),
            funds: [...i.funds || []]
          })
        }));
        const result = await this.signAndBroadcast(senderAddress, msgs, fee, memo);
        if ((0, stargate_1.isDeliverTxFailure)(result)) {
          throw new Error(createDeliverTxResponseErrorMessage(result));
        }
        return {
          logs: stargate_1.logs.parseRawLog(result.rawLog),
          height: result.height,
          transactionHash: result.transactionHash,
          events: result.events,
          gasWanted: result.gasWanted,
          gasUsed: result.gasUsed
        };
      }
      async sendTokens(senderAddress, recipientAddress, amount, fee, memo = "") {
        const sendMsg = {
          typeUrl: "/cosmos.bank.v1beta1.MsgSend",
          value: {
            fromAddress: senderAddress,
            toAddress: recipientAddress,
            amount: [...amount]
          }
        };
        return this.signAndBroadcast(senderAddress, [sendMsg], fee, memo);
      }
      async delegateTokens(delegatorAddress, validatorAddress, amount, fee, memo = "") {
        const delegateMsg = {
          typeUrl: "/cosmos.staking.v1beta1.MsgDelegate",
          value: tx_2.MsgDelegate.fromPartial({ delegatorAddress, validatorAddress, amount })
        };
        return this.signAndBroadcast(delegatorAddress, [delegateMsg], fee, memo);
      }
      async undelegateTokens(delegatorAddress, validatorAddress, amount, fee, memo = "") {
        const undelegateMsg = {
          typeUrl: "/cosmos.staking.v1beta1.MsgUndelegate",
          value: tx_2.MsgUndelegate.fromPartial({ delegatorAddress, validatorAddress, amount })
        };
        return this.signAndBroadcast(delegatorAddress, [undelegateMsg], fee, memo);
      }
      async withdrawRewards(delegatorAddress, validatorAddress, fee, memo = "") {
        const withdrawDelegatorRewardMsg = {
          typeUrl: "/cosmos.distribution.v1beta1.MsgWithdrawDelegatorReward",
          value: tx_1.MsgWithdrawDelegatorReward.fromPartial({ delegatorAddress, validatorAddress })
        };
        return this.signAndBroadcast(delegatorAddress, [withdrawDelegatorRewardMsg], fee, memo);
      }
      /**
       * Creates a transaction with the given messages, fee, memo and timeout height. Then signs and broadcasts the transaction.
       *
       * @param signerAddress The address that will sign transactions using this instance. The signer must be able to sign with this address.
       * @param messages
       * @param fee
       * @param memo
       * @param timeoutHeight (optional) timeout height to prevent the tx from being committed past a certain height
       */
      async signAndBroadcast(signerAddress, messages, fee, memo = "", timeoutHeight) {
        let usedFee;
        if (fee == "auto" || typeof fee === "number") {
          (0, utils_1.assertDefined)(this.gasPrice, "Gas price must be set in the client options when auto gas is used.");
          const gasEstimation = await this.simulate(signerAddress, messages, memo);
          const multiplier = typeof fee === "number" ? fee : this.defaultGasMultiplier;
          usedFee = (0, stargate_1.calculateFee)(Math.round(gasEstimation * multiplier), this.gasPrice);
        } else {
          usedFee = fee;
        }
        const txRaw = await this.sign(signerAddress, messages, usedFee, memo, void 0, timeoutHeight);
        const txBytes = tx_3.TxRaw.encode(txRaw).finish();
        return this.broadcastTx(txBytes, this.broadcastTimeoutMs, this.broadcastPollIntervalMs);
      }
      /**
       * Creates a transaction with the given messages, fee, memo and timeout height. Then signs and broadcasts the transaction.
       *
       * This method is useful if you want to send a transaction in broadcast,
       * without waiting for it to be placed inside a block, because for example
       * I would like to receive the hash to later track the transaction with another tool.
       *
       * @param signerAddress The address that will sign transactions using this instance. The signer must be able to sign with this address.
       * @param messages
       * @param fee
       * @param memo
       * @param timeoutHeight (optional) timeout height to prevent the tx from being committed past a certain height
       *
       * @returns Returns the hash of the transaction
       */
      async signAndBroadcastSync(signerAddress, messages, fee, memo = "", timeoutHeight) {
        let usedFee;
        if (fee == "auto" || typeof fee === "number") {
          (0, utils_1.assertDefined)(this.gasPrice, "Gas price must be set in the client options when auto gas is used.");
          const gasEstimation = await this.simulate(signerAddress, messages, memo);
          const multiplier = typeof fee === "number" ? fee : this.defaultGasMultiplier;
          usedFee = (0, stargate_1.calculateFee)(Math.round(gasEstimation * multiplier), this.gasPrice);
        } else {
          usedFee = fee;
        }
        const txRaw = await this.sign(signerAddress, messages, usedFee, memo, void 0, timeoutHeight);
        const txBytes = tx_3.TxRaw.encode(txRaw).finish();
        return this.broadcastTxSync(txBytes);
      }
      async sign(signerAddress, messages, fee, memo, explicitSignerData, timeoutHeight) {
        let signerData;
        if (explicitSignerData) {
          signerData = explicitSignerData;
        } else {
          const { accountNumber, sequence } = await this.getSequence(signerAddress);
          const chainId = await this.getChainId();
          signerData = {
            accountNumber,
            sequence,
            chainId
          };
        }
        return (0, proto_signing_1.isOfflineDirectSigner)(this.signer) ? this.signDirect(signerAddress, messages, fee, memo, signerData, timeoutHeight) : this.signAmino(signerAddress, messages, fee, memo, signerData, timeoutHeight);
      }
      async signAmino(signerAddress, messages, fee, memo, { accountNumber, sequence, chainId }, timeoutHeight) {
        (0, utils_1.assert)(!(0, proto_signing_1.isOfflineDirectSigner)(this.signer));
        const accountFromSigner = (await this.signer.getAccounts()).find((account) => account.address === signerAddress);
        if (!accountFromSigner) {
          throw new Error("Failed to retrieve account from signer");
        }
        const pubkey = (0, proto_signing_1.encodePubkey)((0, amino_1.encodeSecp256k1Pubkey)(accountFromSigner.pubkey));
        const signMode = signing_1.SignMode.SIGN_MODE_LEGACY_AMINO_JSON;
        const msgs = messages.map((msg) => this.aminoTypes.toAmino(msg));
        const signDoc = (0, amino_1.makeSignDoc)(msgs, fee, chainId, memo, accountNumber, sequence, timeoutHeight);
        const { signature, signed } = await this.signer.signAmino(signerAddress, signDoc);
        const signedTxBody = {
          typeUrl: "/cosmos.tx.v1beta1.TxBody",
          value: {
            messages: signed.msgs.map((msg) => this.aminoTypes.fromAmino(msg)),
            memo: signed.memo,
            timeoutHeight
          }
        };
        const signedTxBodyBytes = this.registry.encode(signedTxBody);
        const signedGasLimit = math_1.Int53.fromString(signed.fee.gas).toNumber();
        const signedSequence = math_1.Int53.fromString(signed.sequence).toNumber();
        const signedAuthInfoBytes = (0, proto_signing_1.makeAuthInfoBytes)([{ pubkey, sequence: signedSequence }], signed.fee.amount, signedGasLimit, signed.fee.granter, signed.fee.payer, signMode);
        return tx_3.TxRaw.fromPartial({
          bodyBytes: signedTxBodyBytes,
          authInfoBytes: signedAuthInfoBytes,
          signatures: [(0, encoding_1.fromBase64)(signature.signature)]
        });
      }
      async signDirect(signerAddress, messages, fee, memo, { accountNumber, sequence, chainId }, timeoutHeight) {
        (0, utils_1.assert)((0, proto_signing_1.isOfflineDirectSigner)(this.signer));
        const accountFromSigner = (await this.signer.getAccounts()).find((account) => account.address === signerAddress);
        if (!accountFromSigner) {
          throw new Error("Failed to retrieve account from signer");
        }
        const pubkey = (0, proto_signing_1.encodePubkey)((0, amino_1.encodeSecp256k1Pubkey)(accountFromSigner.pubkey));
        const txBody = {
          typeUrl: "/cosmos.tx.v1beta1.TxBody",
          value: {
            messages,
            memo,
            timeoutHeight
          }
        };
        const txBodyBytes = this.registry.encode(txBody);
        const gasLimit = math_1.Int53.fromString(fee.gas).toNumber();
        const authInfoBytes = (0, proto_signing_1.makeAuthInfoBytes)([{ pubkey, sequence }], fee.amount, gasLimit, fee.granter, fee.payer);
        const signDoc = (0, proto_signing_1.makeSignDoc)(txBodyBytes, authInfoBytes, chainId, accountNumber);
        const { signature, signed } = await this.signer.signDirect(signerAddress, signDoc);
        return tx_3.TxRaw.fromPartial({
          bodyBytes: signed.bodyBytes,
          authInfoBytes: signed.authInfoBytes,
          signatures: [(0, encoding_1.fromBase64)(signature.signature)]
        });
      }
    };
    exports.SigningCosmWasmClient = SigningCosmWasmClient;
  }
});

// node_modules/@cosmjs/cosmwasm-stargate/build/index.js
var require_build9 = __commonJS({
  "node_modules/@cosmjs/cosmwasm-stargate/build/index.js"(exports) {
    Object.defineProperty(exports, "__esModule", { value: true });
    exports.SigningCosmWasmClient = exports.wasmTypes = exports.setupWasmExtension = exports.isMsgUpdateAdminEncodeObject = exports.isMsgStoreCodeEncodeObject = exports.isMsgMigrateEncodeObject = exports.isMsgInstantiateContractEncodeObject = exports.isMsgInstantiateContract2EncodeObject = exports.isMsgExecuteEncodeObject = exports.isMsgClearAdminEncodeObject = exports.createWasmAminoConverters = exports.instantiate2Address = exports._instantiate2AddressIntermediate = exports.toBinary = exports.fromBinary = exports.CosmWasmClient = void 0;
    var cosmwasmclient_1 = require_cosmwasmclient();
    Object.defineProperty(exports, "CosmWasmClient", { enumerable: true, get: function() {
      return cosmwasmclient_1.CosmWasmClient;
    } });
    var encoding_1 = require_encoding();
    Object.defineProperty(exports, "fromBinary", { enumerable: true, get: function() {
      return encoding_1.fromBinary;
    } });
    Object.defineProperty(exports, "toBinary", { enumerable: true, get: function() {
      return encoding_1.toBinary;
    } });
    var instantiate2_1 = require_instantiate2();
    Object.defineProperty(exports, "_instantiate2AddressIntermediate", { enumerable: true, get: function() {
      return instantiate2_1._instantiate2AddressIntermediate;
    } });
    Object.defineProperty(exports, "instantiate2Address", { enumerable: true, get: function() {
      return instantiate2_1.instantiate2Address;
    } });
    var modules_1 = require_modules();
    Object.defineProperty(exports, "createWasmAminoConverters", { enumerable: true, get: function() {
      return modules_1.createWasmAminoConverters;
    } });
    Object.defineProperty(exports, "isMsgClearAdminEncodeObject", { enumerable: true, get: function() {
      return modules_1.isMsgClearAdminEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgExecuteEncodeObject", { enumerable: true, get: function() {
      return modules_1.isMsgExecuteEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgInstantiateContract2EncodeObject", { enumerable: true, get: function() {
      return modules_1.isMsgInstantiateContract2EncodeObject;
    } });
    Object.defineProperty(exports, "isMsgInstantiateContractEncodeObject", { enumerable: true, get: function() {
      return modules_1.isMsgInstantiateContractEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgMigrateEncodeObject", { enumerable: true, get: function() {
      return modules_1.isMsgMigrateEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgStoreCodeEncodeObject", { enumerable: true, get: function() {
      return modules_1.isMsgStoreCodeEncodeObject;
    } });
    Object.defineProperty(exports, "isMsgUpdateAdminEncodeObject", { enumerable: true, get: function() {
      return modules_1.isMsgUpdateAdminEncodeObject;
    } });
    Object.defineProperty(exports, "setupWasmExtension", { enumerable: true, get: function() {
      return modules_1.setupWasmExtension;
    } });
    Object.defineProperty(exports, "wasmTypes", { enumerable: true, get: function() {
      return modules_1.wasmTypes;
    } });
    var signingcosmwasmclient_1 = require_signingcosmwasmclient();
    Object.defineProperty(exports, "SigningCosmWasmClient", { enumerable: true, get: function() {
      return signingcosmwasmclient_1.SigningCosmWasmClient;
    } });
  }
});
export default require_build9();
//# sourceMappingURL=@cosmjs_cosmwasm-stargate.js.map
